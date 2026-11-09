//! Sorting files into folders by date, letter, place, type or camera.

use crate::fsutil::{self, Kind, KindFilter};
use crate::i18n::t;
use crate::job::{Progress, Report};
use crate::meta::{self, Meta};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum Scheme {
    #[default]
    YearMonth,
    YearMonthDay,
    Year,
    Alphabet,
    Location,
    YearLocation,
    Type,
    Camera,
}

impl Scheme {
    pub const ALL: [Scheme; 8] = [
        Scheme::YearMonth,
        Scheme::YearMonthDay,
        Scheme::Year,
        Scheme::Location,
        Scheme::YearLocation,
        Scheme::Alphabet,
        Scheme::Type,
        Scheme::Camera,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Scheme::YearMonth => t("sch_ym"),
            Scheme::YearMonthDay => t("sch_ymd"),
            Scheme::Year => t("sch_y"),
            Scheme::Alphabet => t("sch_abc"),
            Scheme::Location => t("sch_loc"),
            Scheme::YearLocation => t("sch_yloc"),
            Scheme::Type => t("sch_type"),
            Scheme::Camera => t("sch_cam"),
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Scheme::YearMonth | Scheme::YearMonthDay | Scheme::Year => "📅",
            Scheme::Alphabet => "🔤",
            Scheme::Location | Scheme::YearLocation => "🌍",
            Scheme::Type => "🗂",
            Scheme::Camera => "📷",
        }
    }
    pub fn example(self) -> String {
        let m = month_name(7);
        let sep = std::path::MAIN_SEPARATOR;
        match self {
            Scheme::YearMonth => format!("2024{sep}07 - {m}"),
            Scheme::YearMonthDay => format!("2024{sep}07 - {m}{sep}2024-07-15"),
            Scheme::Year => "2024".into(),
            Scheme::Alphabet => format!("A, B, C … {sep} #"),
            Scheme::Location => format!("{}{sep}{}", meta::country_name("ME"), "Budva"),
            Scheme::YearLocation => format!("2024{sep}{} - Budva", meta::country_name("ME")),
            Scheme::Type => format!("{}{sep}{}", t("type_images"), t("type_videos")),
            Scheme::Camera => format!("Apple iPhone 13{sep}Samsung SM-A525F"),
        }
    }
    pub fn needs_meta(self) -> bool {
        !matches!(self, Scheme::Alphabet | Scheme::Type)
    }
}

pub fn month_name(m: u32) -> &'static str {
    const KEYS: [&str; 12] = ["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10", "m11", "m12"];
    t(KEYS[(m.clamp(1, 12) - 1) as usize])
}

fn type_folder(k: Kind) -> &'static str {
    match k {
        Kind::Image => t("type_images"),
        Kind::Video => t("type_videos"),
        Kind::Audio => t("type_audio"),
        Kind::Document => t("type_docs"),
        Kind::Other => t("type_other"),
    }
}

/// Relative folder for a file under the given scheme.
pub fn folder_for(scheme: Scheme, path: &Path, kind: Kind, m: &Meta) -> PathBuf {
    let mut out = PathBuf::new();
    let date = m.date.as_ref().map(meta::year_month);
    match scheme {
        Scheme::YearMonth | Scheme::YearMonthDay | Scheme::Year => match date {
            Some((y, mo, d)) => {
                out.push(y.to_string());
                if scheme != Scheme::Year {
                    out.push(format!("{:02} - {}", mo, month_name(mo)));
                }
                if scheme == Scheme::YearMonthDay {
                    out.push(format!("{y}-{mo:02}-{d:02}"));
                }
            }
            None => out.push(fsutil::sanitize(t("no_date"))),
        },
        Scheme::Alphabet => {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let c = name.chars().find(|c| c.is_alphanumeric());
            match c {
                Some(c) if c.is_alphabetic() => out.push(c.to_uppercase().collect::<String>()),
                _ => out.push("#"),
            }
        }
        Scheme::Location | Scheme::YearLocation => {
            let place = m.gps.map(|(la, lo)| meta::place(la, lo));
            if scheme == Scheme::YearLocation {
                let y = date.map(|d| d.0.to_string()).unwrap_or_else(|| t("no_date").to_string());
                out.push(fsutil::sanitize(&y));
                match place {
                    Some(p) => out.push(fsutil::sanitize(&format!("{} - {}", p.country, p.city))),
                    None => out.push(fsutil::sanitize(t("no_location"))),
                }
            } else {
                match place {
                    Some(p) => {
                        out.push(fsutil::sanitize(&p.country));
                        out.push(fsutil::sanitize(&p.city));
                    }
                    None => out.push(fsutil::sanitize(t("no_location"))),
                }
            }
        }
        Scheme::Type => out.push(fsutil::sanitize(type_folder(kind))),
        Scheme::Camera => match &m.camera {
            Some(c) => out.push(fsutil::sanitize(c)),
            None => out.push(fsutil::sanitize(t("no_camera"))),
        },
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum OrgMode {
    #[default]
    MoveInPlace,
    CopyTo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
    #[serde(skip, default)]
    pub size: u64,
}

pub struct OrgPlan {
    pub root: PathBuf,
    pub dest_root: PathBuf,
    pub mode: OrgMode,
    pub scheme: Scheme,
    pub moves: Vec<Move>,
    pub in_place: usize,
    /// (relative folder, file count, bytes)
    pub folders: Vec<(String, usize, u64)>,
    pub total_bytes: u64,
}

struct Found {
    path: PathBuf,
    size: u64,
    mtime: SystemTime,
    kind: Kind,
}

pub fn plan(
    root: &Path,
    dest: Option<&Path>,
    scheme: Scheme,
    mode: OrgMode,
    kinds: KindFilter,
    skip_hidden: bool,
    p: &Progress,
) -> Result<OrgPlan, String> {
    let dest_root = match mode {
        OrgMode::MoveInPlace => root.to_path_buf(),
        OrgMode::CopyTo => dest.ok_or_else(|| t("err_no_dest").to_string())?.to_path_buf(),
    };
    p.stage("st_scanning", 0);
    let mut files = Vec::new();
    let walker = walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
        if e.depth() == 0 {
            return true;
        }
        let n = e.file_name().to_string_lossy();
        if e.file_type().is_dir() {
            if fsutil::is_skipped_dir(&n) {
                return false;
            }
            // don't walk into the destination when copying inside the same tree
            if mode == OrgMode::CopyTo && e.path() == dest_root {
                return false;
            }
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
        let kind = fsutil::kind_of(e.path());
        if !kinds.accepts(kind) {
            continue;
        }
        if let Ok(md) = e.metadata() {
            files.push(Found { path: e.path().to_path_buf(), size: md.len(), mtime: fsutil::mtime_of(&md), kind });
            p.tick();
            if files.len() % 200 == 0 {
                p.set_current(e.path().to_string_lossy());
            }
        }
    }

    p.stage("st_reading_meta", files.len() as u64);
    let folders: Vec<PathBuf> = files
        .par_iter()
        .map(|f| {
            if p.cancelled() {
                return PathBuf::new();
            }
            let m = if scheme.needs_meta() { meta::read_meta(&f.path, f.kind, f.mtime) } else { Meta::default() };
            p.tick();
            folder_for(scheme, &f.path, f.kind, &m)
        })
        .collect();
    if p.cancelled() {
        return Err(t("cancelled").into());
    }

    let mut taken: HashSet<String> = HashSet::new();
    let mut moves = Vec::new();
    let mut in_place = 0;
    let mut summary: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let mut total_bytes = 0;
    for (f, rel) in files.iter().zip(folders) {
        let target_dir = dest_root.join(&rel);
        let e = summary.entry(rel.to_string_lossy().to_string()).or_default();
        e.0 += 1;
        e.1 += f.size;
        if mode == OrgMode::MoveInPlace && f.path.parent() == Some(target_dir.as_path()) {
            in_place += 1;
            taken.insert(fsutil::path_key(&f.path));
            continue;
        }
        let name = f.path.file_name().unwrap_or_default();
        let want = target_dir.join(name);
        let to = fsutil::unique_path(&want, &|c| taken.contains(&fsutil::path_key(c)));
        taken.insert(fsutil::path_key(&to));
        total_bytes += f.size;
        moves.push(Move { from: f.path.clone(), to, size: f.size });
    }
    Ok(OrgPlan {
        root: root.to_path_buf(),
        dest_root,
        mode,
        scheme,
        moves,
        in_place,
        folders: summary.into_iter().map(|(k, (n, b))| (k, n, b)).collect(),
        total_bytes,
    })
}

#[derive(Serialize, Deserialize)]
pub struct Journal {
    pub root: PathBuf,
    pub when: String,
    pub moves: Vec<Move>,
}

fn journal_dir() -> PathBuf {
    crate::settings::data_dir().join("undo")
}

/// Runs the plan. For moves a journal is written so the change can be undone.
pub fn execute(plan: &OrgPlan, p: &Progress) -> (Report, Option<PathBuf>) {
    let mut rep = Report { open_path: Some(plan.dest_root.clone()), ..Default::default() };
    p.stage("st_organizing", plan.moves.len() as u64);
    p.bytes.store(0, Ordering::Relaxed);
    p.bytes_total.store(plan.total_bytes, Ordering::Relaxed);
    let mut done: Vec<Move> = Vec::new();
    let mut parents = Vec::new();
    for m in &plan.moves {
        if p.cancelled() {
            rep.cancelled = true;
            break;
        }
        p.set_current(m.from.to_string_lossy());
        let res = match plan.mode {
            OrgMode::MoveInPlace => fsutil::move_file(&m.from, &m.to, &p.bytes, &p.cancel),
            OrgMode::CopyTo => {
                if m.to.exists() {
                    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "exists"))
                } else {
                    fsutil::copy_file(&m.from, &m.to, &p.bytes, &p.cancel)
                }
            }
        };
        match res {
            Ok(()) => {
                rep.ok_files += 1;
                rep.ok_bytes += m.size;
                if plan.mode == OrgMode::MoveInPlace {
                    if let Some(par) = m.from.parent() {
                        parents.push(par.to_path_buf());
                    }
                    done.push(m.clone());
                }
            }
            Err(e) => rep.errors.push((m.from.to_string_lossy().into(), e.to_string())),
        }
        p.tick();
    }
    let mut journal_path = None;
    if plan.mode == OrgMode::MoveInPlace {
        fsutil::remove_empty_dirs(parents, &plan.root);
        if !done.is_empty() {
            let _ = std::fs::create_dir_all(journal_dir());
            let now = chrono::Local::now();
            let j = Journal { root: plan.root.clone(), when: now.format("%d.%m.%Y %H:%M").to_string(), moves: done };
            let path = journal_dir().join(format!("{}.json", now.format("%Y%m%d-%H%M%S")));
            if let Ok(txt) = serde_json::to_string(&j) {
                if std::fs::write(&path, txt).is_ok() {
                    journal_path = Some(path);
                }
            }
        }
    }
    (rep, journal_path)
}

pub fn load_journal(path: &Path) -> Option<Journal> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn undo(path: &Path, p: &Progress) -> Report {
    let mut rep = Report::default();
    let Some(j) = load_journal(path) else {
        rep.errors.push((path.to_string_lossy().into(), t("err_journal").into()));
        return rep;
    };
    rep.open_path = Some(j.root.clone());
    p.stage("st_undoing", j.moves.len() as u64);
    let mut parents = Vec::new();
    for m in j.moves.iter().rev() {
        if p.cancelled() {
            rep.cancelled = true;
            break;
        }
        p.set_current(m.to.to_string_lossy());
        if m.from.exists() {
            rep.errors.push((m.from.to_string_lossy().into(), t("err_exists").into()));
        } else {
            match fsutil::move_file(&m.to, &m.from, &p.bytes, &p.cancel) {
                Ok(()) => {
                    rep.ok_files += 1;
                    if let Some(par) = m.to.parent() {
                        parents.push(par.to_path_buf());
                    }
                }
                Err(e) => rep.errors.push((m.to.to_string_lossy().into(), e.to_string())),
            }
        }
        p.tick();
    }
    fsutil::remove_empty_dirs(parents, &j.root);
    if !rep.cancelled {
        let _ = std::fs::remove_file(path);
    }
    rep
}
