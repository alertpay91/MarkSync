//! Sorting files into folders by date, letter, place, type or camera.

use crate::fsutil::{self, Kind, KindFilter};
use crate::i18n::t;
use crate::job::{Progress, Report};
use crate::meta::{self, Meta};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
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

/// Extra choices that apply on top of a scheme.
#[derive(Clone, Copy, Debug, Default)]
pub struct OrgOptions {
    /// keep meaningful folder names as a sub-folder ("2024\07 - Jul\Rođendan Mila")
    pub keep_albums: bool,
    /// photos/videos whose date is only the file's modification time go to "_Proveri datum"
    pub flag_dates: bool,
    /// identical files are moved/copied once; extra copies go to the Recycle Bin
    pub remove_dups: bool,
}

impl Scheme {
    fn is_date_based(self) -> bool {
        matches!(self, Scheme::YearMonth | Scheme::YearMonthDay | Scheme::Year | Scheme::YearLocation)
    }
}

/// `folder_for` plus the album and "check the date" options.
pub fn folder_with(
    scheme: Scheme,
    path: &Path,
    kind: Kind,
    m: &Meta,
    album: Option<&str>,
    opts: OrgOptions,
) -> (PathBuf, bool) {
    let mut out = PathBuf::new();
    let uncertain = opts.flag_dates
        && scheme.is_date_based()
        && matches!(kind, Kind::Image | Kind::Video)
        && m.date_from == meta::DateFrom::FileTime;
    if uncertain {
        out.push(fsutil::sanitize(&format!("_{}", t("check_date"))));
    }
    out.push(folder_for(scheme, path, kind, m));
    if opts.keep_albums {
        if let Some(a) = album {
            out.push(a);
        }
    }
    (out, uncertain)
}

/// Words that say nothing about the event in a folder name.
const GENERIC: &[&str] = &[
    "dcim", "camera", "kamera", "cam", "100andro", "100media", "100apple", "101apple", "pictures", "picture",
    "photos", "photo", "images", "image", "img", "slike", "slika", "fotografije", "fotografija", "fotke", "foto",
    "new", "folder", "nova", "novi", "fascikla", "backup", "bekap", "download", "downloads", "preuzimanja",
    "preuzeto", "telefon", "telefona", "telefonu", "phone", "mobile", "mobilni", "mobilnog", "screenshots",
    "screenshot", "snimci", "ekrana", "whatsapp", "viber", "telegram", "instagram", "facebook", "messenger",
    "sent", "private", "media", "video", "videos", "movies", "filteri", "filter", "filters", "edited",
    "izmenjeno", "izmenjene", "copy", "kopija", "razno", "ostalo", "misc", "temp", "tmp", "sdcard", "card",
    "internal", "shared", "storage", "interna", "memorija", "snapseed", "vsco", "lightroom", "vivid", "warm",
    "cool", "mono", "noir", "portrait", "export", "exports", "raw", "jpg", "jpeg", "heic", "the", "and",
    "moje", "moji", "sve", "all", "stare", "stari", "nove", "old", "users", "user", "desktop", "documents",
    "dokumenti", "radna", "povrsina", "površina", "galerija", "gallery", "album", "albums", "export",
    "onedrive", "google", "icloud", "samsung", "pixel", "iphone", "android", "huawei", "xiaomi",
];
const MONTHS: &[&str] = &[
    "januar", "februar", "mart", "april", "maj", "jun", "juni", "jul", "juli", "avgust", "august", "septembar",
    "oktobar", "novembar", "decembar", "january", "february", "march", "may", "june", "july", "september",
    "october", "november", "december", "jan", "feb", "mar", "apr", "jun", "aug", "avg", "sep", "sept", "oct",
    "okt", "nov", "dec", "januara", "februara", "marta", "aprila", "maja", "juna", "jula", "avgusta",
    "septembra", "oktobra", "novembra", "decembra",
];

/// True for names like "Rođendan Mila" or "More Budva", false for "Nova fascikla (3)",
/// "Camera", "2024-07", "Slike sa telefona 14 januar 2026" or "SM-G990B_20251211193024".
pub fn is_meaningful_folder(name: &str) -> bool {
    let lower = name.to_lowercase();
    // device dumps / backups with long numbers
    let mut run = 0;
    for c in lower.chars() {
        run = if c.is_ascii_digit() { run + 1 } else { 0 };
        if run >= 6 {
            return false;
        }
    }
    // our own folders ("Bez datuma", "_Proveri datum", type folders…)
    let own: Vec<String> = ["no_date", "no_location", "no_camera", "check_date", "type_images", "type_videos", "type_audio", "type_docs", "type_other"]
        .iter()
        .map(|k| t(k).to_lowercase())
        .collect();
    let trimmed = lower.trim_start_matches('_');
    if own.iter().any(|o| o == trimmed) {
        return false;
    }
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
        .any(|w| !GENERIC.contains(&w) && !MONTHS.contains(&w) && !own.iter().any(|o| o.split(' ').any(|x| x == w)))
}

/// The closest meaningful folder name between `root` and the file.
pub fn album_for(root: &Path, file: &Path) -> Option<String> {
    album_in(file.parent()?.strip_prefix(root).ok()?)
}

/// The deepest meaningful folder name in a relative folder path.
pub fn album_in(rel: &Path) -> Option<String> {
    let names: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    names.into_iter().rev().find(|n| is_meaningful_folder(n)).map(|n| fsutil::sanitize(&n))
}

#[derive(Clone, Debug)]
pub struct DupExtra {
    pub path: PathBuf,
    pub kept: PathBuf,
    pub size: u64,
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
    /// extra identical copies (not moved; go to the Recycle Bin when moving in place)
    pub dups: Vec<DupExtra>,
    /// files put in "_Proveri datum"
    pub uncertain: usize,
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
    opts: OrgOptions,
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

    // identical files: keep the best-named one, the others are extras
    let mut extra_of: HashMap<usize, usize> = HashMap::new();
    if opts.remove_dups {
        let list: Vec<(PathBuf, u64, SystemTime)> = files.iter().map(|f| (f.path.clone(), f.size, f.mtime)).collect();
        for group in crate::dupes::identical_groups(&list, p)? {
            let keep = *group
                .iter()
                .min_by(|&&a, &&b| crate::dupes::original_order(&list[a].0, list[a].2, &list[b].0, list[b].2))
                .unwrap();
            for &i in &group {
                if i != keep {
                    extra_of.insert(i, keep);
                }
            }
        }
    }

    p.stage("st_reading_meta", files.len() as u64);
    let folders: Vec<(PathBuf, bool)> = files
        .par_iter()
        .enumerate()
        .map(|(i, f)| {
            if p.cancelled() || extra_of.contains_key(&i) {
                return (PathBuf::new(), false);
            }
            let m = if scheme.needs_meta() { meta::read_meta(&f.path, f.kind, f.mtime) } else { Meta::default() };
            p.tick();
            let album = if opts.keep_albums { album_for(root, &f.path) } else { None };
            folder_with(scheme, &f.path, f.kind, &m, album.as_deref(), opts)
        })
        .collect();
    if p.cancelled() {
        return Err(t("cancelled").into());
    }

    let mut taken: HashSet<String> = HashSet::new();
    let mut moves = Vec::new();
    let mut in_place = 0;
    let mut uncertain = 0;
    let mut dups = Vec::new();
    let mut summary: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let mut total_bytes = 0;
    for (i, (f, (rel, unsure))) in files.iter().zip(folders).enumerate() {
        if let Some(&k) = extra_of.get(&i) {
            dups.push(DupExtra { path: f.path.clone(), kept: files[k].path.clone(), size: f.size });
            continue;
        }
        if unsure {
            uncertain += 1;
        }
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
    // the kept copy is usually moved too: check against its new place when removing extras
    let new_place: HashMap<String, PathBuf> = moves.iter().map(|m| (fsutil::path_key(&m.from), m.to.clone())).collect();
    if mode == OrgMode::MoveInPlace {
        for d in &mut dups {
            if let Some(to) = new_place.get(&fsutil::path_key(&d.kept)) {
                d.kept = to.clone();
            }
        }
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
        dups,
        uncertain,
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
    // the undo record is written while moving, so even a crash or power cut can be undone
    let now = chrono::Local::now();
    let journal_file = journal_dir().join(format!("{}.jsonl", now.format("%Y%m%d-%H%M%S")));
    let when = now.format("%d.%m.%Y %H:%M").to_string();
    let mut journal: Option<std::fs::File> = None;
    let mut append = |m: &Move| {
        use std::io::Write;
        if plan.mode != OrgMode::MoveInPlace {
            return;
        }
        if journal.is_none() {
            let _ = std::fs::create_dir_all(journal_dir());
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&journal_file) {
                let head = serde_json::json!({ "root": plan.root, "when": when });
                let _ = writeln!(f, "{head}");
                journal = Some(f);
            }
        }
        if let Some(f) = journal.as_mut() {
            if let Ok(line) = serde_json::to_string(m) {
                let _ = writeln!(f, "{line}");
                let _ = f.flush();
            }
        }
    };
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
                    fsutil::log(t("log_moved"), &m.from, Some(&m.to));
                    if let Some(par) = m.from.parent() {
                        parents.push(par.to_path_buf());
                    }
                    append(m);
                    done.push(m.clone());
                } else {
                    fsutil::log(t("log_copied"), &m.from, Some(&m.to));
                }
            }
            Err(e) => rep.errors.push((m.from.to_string_lossy().into(), e.to_string())),
        }
        p.tick();
    }
    // extra identical copies: Recycle Bin, or the side folder on disks without one
    if plan.mode == OrgMode::MoveInPlace && !rep.cancelled {
        p.stage("st_removing", plan.dups.len() as u64);
        for d in &plan.dups {
            if p.cancelled() {
                rep.cancelled = true;
                break;
            }
            p.set_current(d.path.to_string_lossy());
            if let Err(e) = fsutil::safe_to_remove_extra(&d.path, &d.kept, &p.cancel) {
                rep.errors.push((d.path.to_string_lossy().into(), e));
                p.tick();
                continue;
            }
            let binned = trash::delete(&d.path).is_ok() && !d.path.exists();
            if binned {
                fsutil::log(t("log_dup_bin"), &d.path, Some(&d.kept));
            }
            let res = if binned {
                Ok(())
            } else {
                let rel = d.path.strip_prefix(&plan.root).unwrap_or(&d.path);
                let to = fsutil::unique_path(&plan.root.join(fsutil::DUP_FOLDER).join(rel), &|_| false);
                let r = fsutil::move_file(&d.path, &to, &p.bytes, &p.cancel);
                if r.is_ok() {
                    fsutil::log(t("log_moved"), &d.path, Some(&to));
                    let mv = Move { from: d.path.clone(), to, size: d.size };
                    append(&mv);
                    done.push(mv);
                }
                r
            };
            match res {
                Ok(()) => {
                    rep.dups_removed += 1;
                    if let Some(par) = d.path.parent() {
                        parents.push(par.to_path_buf());
                    }
                }
                Err(e) => rep.errors.push((d.path.to_string_lossy().into(), e.to_string())),
            }
            p.tick();
        }
    }
    let mut journal_path = None;
    if plan.mode == OrgMode::MoveInPlace {
        fsutil::remove_empty_dirs(parents, &plan.root);
        drop(append);
        if !done.is_empty() && journal_file.exists() {
            journal_path = Some(journal_file.clone());
        }
    }
    (rep, journal_path)
}

/// The newest undo record (also one left behind by a crash).
pub fn latest_journal() -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(journal_dir())
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("json" | "jsonl")))
        .collect();
    v.sort();
    v.pop()
}

pub fn load_journal(path: &Path) -> Option<Journal> {
    let txt = std::fs::read_to_string(path).ok()?;
    if let Ok(j) = serde_json::from_str::<Journal>(&txt) {
        return Some(j); // older single-JSON format
    }
    let mut lines = txt.lines().filter(|l| !l.trim().is_empty());
    let head: serde_json::Value = serde_json::from_str(lines.next()?).ok()?;
    let root = PathBuf::from(head.get("root")?.as_str()?);
    let when = head.get("when").and_then(|w| w.as_str()).unwrap_or("").to_string();
    // a line cut off by a crash is simply ignored
    let moves = lines.filter_map(|l| serde_json::from_str::<Move>(l).ok()).collect();
    Some(Journal { root, when, moves })
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
    // moves that could not be undone stay in the record, so they can be retried
    let mut left: Vec<Move> = Vec::new();
    for m in j.moves.iter().rev() {
        if p.cancelled() {
            rep.cancelled = true;
            left.push(m.clone());
            continue;
        }
        p.set_current(m.to.to_string_lossy());
        if m.from.exists() {
            rep.errors.push((m.from.to_string_lossy().into(), t("err_exists").into()));
            left.push(m.clone());
        } else {
            match fsutil::move_file(&m.to, &m.from, &p.bytes, &p.cancel) {
                Ok(()) => {
                    fsutil::log(t("log_undone"), &m.to, Some(&m.from));
                    rep.ok_files += 1;
                    if let Some(par) = m.to.parent() {
                        parents.push(par.to_path_buf());
                    }
                }
                Err(e) => {
                    rep.errors.push((m.to.to_string_lossy().into(), e.to_string()));
                    left.push(m.clone());
                }
            }
        }
        p.tick();
    }
    fsutil::remove_empty_dirs(parents, &j.root);
    if left.is_empty() {
        let _ = std::fs::remove_file(path);
    } else {
        left.reverse();
        let rest = Journal { root: j.root.clone(), when: j.when.clone(), moves: left };
        if let Ok(txt) = serde_json::to_string(&rest) {
            let _ = std::fs::write(path, txt);
        }
    }
    rep
}
