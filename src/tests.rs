//! End-to-end tests on the scenario from tools/make_testdata.py
//! (set MARKSYNC_TESTDATA to its folder; a fresh copy is used for each test).

use crate::fsutil::KindFilter;
use crate::job::Progress;
use crate::organize::{self, OrgMode, Scheme};
use crate::settings::LayoutChoice;
use crate::sync::{self, Source, Status};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn fresh(name: &str) -> Option<PathBuf> {
    let src = PathBuf::from(std::env::var("MARKSYNC_TESTDATA").ok()?);
    let dst = src.with_file_name(format!("run-{name}"));
    let _ = std::fs::remove_dir_all(&dst);
    copy_dir(&src, &dst);
    Some(dst)
}

fn copy_dir(a: &Path, b: &Path) {
    std::fs::create_dir_all(b).unwrap();
    for e in std::fs::read_dir(a).unwrap().flatten() {
        let to = b.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), &to).unwrap();
            let m = std::fs::metadata(e.path()).unwrap();
            filetime::set_file_mtime(&to, filetime::FileTime::from_last_modification_time(&m)).unwrap();
        }
    }
}

fn statuses(items: &[sync::Item], root: &Path) -> HashMap<String, Status> {
    items
        .iter()
        .map(|i| (i.src.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), i.status))
        .collect()
}

fn input(base: &Path, layout: LayoutChoice, excluded: &[&str]) -> sync::CompareInput {
    let root = base.join("Laptop").join("Slike");
    let states = excluded.iter().map(|e| (root.join(e), false)).collect();
    sync::CompareInput {
        sources: vec![Source { root, states }],
        phones: Vec::new(),
        target: base.join("HDD"),
        kinds: KindFilter::default(),
        layout,
        scheme: Scheme::YearMonth,
        skip_hidden: true,
        org_opts: organize::OrgOptions::default(),
    }
}

#[test]
fn compare_copy_recompare() {
    let Some(base) = fresh("sync") else { return };
    let p = Progress::default();
    let inp = input(&base, LayoutChoice::Keep, &["Privatno"]);
    let items = sync::compare(&inp, &p).unwrap();
    let st = statuses(&items, &inp.sources[0].root);
    for (k, v) in &st {
        println!("{v:?}\t{k}");
    }
    assert_eq!(st["Leto 2024/IMG_0001.jpg"], Status::Exists, "same content under another name on HDD");
    assert_eq!(st["Filteri/Vivid/IMG_0002.jpg"], Status::New);
    assert_eq!(st["Leto 2024/IMG_0002.jpg"], Status::DupInSource);
    assert_eq!(st["Leto 2024/IMG_0003.jpg"], Status::Conflict);
    assert_eq!(st["Leto 2024/video.mp4"], Status::New);
    assert_eq!(st["Rim/DSC_100.jpg"], Status::Exists);
    assert_eq!(st["Rim/DSC_101.jpg"], Status::Newer);
    assert_eq!(st["Viber/IMG-20240101-WA0001.jpg"], Status::New);
    assert!(!st.contains_key("Privatno/tajna.jpg"), "unticked folder must be skipped");
    assert!(!st.keys().any(|k| k.contains("Thumbs.db")));
    let dup_pair = [st["Rodjendan/slika.jpg"], st["Rodjendan/slika kopija.jpg"]];
    assert!(dup_pair.contains(&Status::New) && dup_pair.contains(&Status::DupInSource));

    let rep = sync::copy(&items, &p);
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert_eq!(rep.ok_files, 6);
    // conflict → both kept, newer → old one retired (Recycle Bin or renamed)
    let hdd = base.join("HDD");
    assert!(hdd.join("Leto 2024/IMG_0003.jpg").exists());
    assert!(hdd.join("Leto 2024/IMG_0003 (2).jpg").exists());
    assert!(hdd.join("Rim/DSC_101.jpg").exists());

    let again = sync::compare(&inp, &p).unwrap();
    let copyable: Vec<_> = again.iter().filter(|i| i.status.copyable()).map(|i| i.src.display().to_string()).collect();
    assert!(copyable.is_empty(), "after copying nothing should be left: {copyable:?}");
}

#[test]
fn organized_layout_and_flat() {
    let Some(base) = fresh("layout") else { return };
    let p = Progress::default();
    let mut inp = input(&base, LayoutChoice::Organized, &[]);
    inp.scheme = Scheme::Location;
    let items = sync::compare(&inp, &p).unwrap();
    let viber = items.iter().find(|i| i.src.ends_with("IMG-20240101-WA0001.jpg")).unwrap();
    assert!(viber.dest.to_string_lossy().contains("Bez lokacije"), "{}", viber.dest.display());
    let vivid = items.iter().find(|i| i.src.ends_with("Vivid/IMG_0002.jpg")).unwrap();
    println!("vivid → {}", vivid.dest.display());
    assert!(vivid.dest.to_string_lossy().contains("Crna Gora"), "{}", vivid.dest.display());

    inp.layout = LayoutChoice::Flat;
    let items = sync::compare(&inp, &p).unwrap();
    for i in items.iter().filter(|i| i.status.copyable()) {
        assert_eq!(i.dest.parent().unwrap(), base.join("HDD"), "{}", i.dest.display());
    }
}

#[test]
fn organize_in_place_and_undo() {
    let Some(base) = fresh("org") else { return };
    let p = Progress::default();
    let root = base.join("Laptop").join("Slike");
    let before = count_files(&root);
    let plan = organize::plan(&root, None, Scheme::YearMonth, OrgMode::MoveInPlace, KindFilter::default(), true, organize::OrgOptions::default(), &p).unwrap();
    for (f, n, _) in &plan.folders {
        println!("{f}: {n}");
    }
    assert!(plan.folders.iter().any(|(f, _, _)| f.contains("2024") && f.contains("Jul")));
    assert!(plan.folders.iter().any(|(f, _, _)| f.starts_with("2024") && f.contains("Januar")), "date from Viber name");
    let (rep, journal) = organize::execute(&plan, &p);
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert!(root.join("2024").join("07 - Jul").join("IMG_0001.jpg").exists());
    assert!(!root.join("Rim").exists(), "emptied folders are removed");
    assert_eq!(count_files(&root), before);

    let rep = organize::undo(&journal.unwrap(), &p);
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert!(root.join("Rim").join("DSC_100.jpg").exists());
    assert!(!root.join("2024").exists());
    assert_eq!(count_files(&root), before);
}

#[test]
fn duplicates() {
    let Some(base) = fresh("dup") else { return };
    let p = Progress::default();
    let groups = crate::dupes::find(&[base.join("HDD")], KindFilter::default(), true, &p).unwrap();
    let g = groups.iter().find(|g| g.files.len() == 3).expect("group of 3");
    assert!(g.files[g.keep].path.ends_with("a.jpg"), "keeps the original name");
    let rep = crate::dupes::remove_extras(
        std::slice::from_ref(g),
        &[base.join("HDD")],
        crate::settings::DupAction::MoveToFolder,
        &p,
    );
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert!(base.join("HDD").join(crate::fsutil::DUP_FOLDER).join("Dupli").join("b (1).jpg").exists());
}

#[test]
fn names_and_script() {
    use crate::meta::date_from_name;
    assert_eq!(date_from_name("IMG_20240715_101500.jpg").unwrap().to_string(), "2024-07-15 10:15:00");
    assert_eq!(date_from_name("IMG-20240101-WA0001.jpg").unwrap().to_string(), "2024-01-01 12:00:00");
    assert_eq!(date_from_name("2023-05-02 Rim.jpg").unwrap().to_string(), "2023-05-02 12:00:00");
    assert!(date_from_name("IMG_0001.jpg").is_none());
    assert_eq!(crate::i18n::to_cyrillic("Ljubljana, Njegoš, džem — MarkSync GB"), "Љубљана, Његош, џем — MarkSync GB");
    crate::i18n::set_lang(crate::i18n::Lang::SrCyrl);
    assert_eq!(crate::i18n::tf("selected_sum", &["6", "651 KB"]), "Изабрано: 6 фајлова · 651 KB");
    crate::i18n::set_lang(crate::i18n::Lang::SrLatn);
}

fn count_files(p: &Path) -> usize {
    // Thumbs.db & co. are deliberately removed from emptied folders
    walkdir::WalkDir::new(p)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file() && !crate::fsutil::is_junk(&e.file_name().to_string_lossy()))
        .count()
}

/// Real phone, read-only: MARKSYNC_PHONE=1 cargo test --release phone_read -- --nocapture
#[test]
fn phone_read() {
    if std::env::var("MARKSYNC_PHONE").is_err() {
        return;
    }
    use crate::phone;
    let t0 = std::time::Instant::now();
    let devs = phone::list_devices();
    println!("devices: {:?}", devs.iter().map(|d| &d.name).collect::<Vec<_>>());
    let dev = &devs[0];
    let storages = phone::children(&dev.id, phone::WPD_DEVICE_OBJECT_ID_STR).unwrap();
    println!("storages: {:?}", storages.iter().map(|e| (&e.name, e.is_folder)).collect::<Vec<_>>());
    let path = vec![storages[0].name.clone(), "DCIM".into(), "Camera".into()];
    let cam = phone::resolve(&dev.id, &path).unwrap();
    println!("camera obj {cam} ({:?})", t0.elapsed());
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut files = Vec::new();
    phone::walk(&dev.id, &cam, &cancel, &mut |rel, e| files.push((rel, e))).unwrap();
    println!("files: {} in {:?}", files.len(), t0.elapsed());
    for (rel, e) in files.iter().take(4) {
        println!("  {} size={} mtime={:?}", rel.display(), e.size, e.mtime.map(crate::i18n::fmt_date));
    }
    let zero = files.iter().filter(|(_, e)| e.size == 0).count();
    let nodate = files.iter().filter(|(_, e)| e.mtime.is_none()).count();
    println!("size 0: {zero}, no date: {nodate}");
    let (_, e) = files.iter().find(|(r, _)| r.to_string_lossy().ends_with(".jpg")).unwrap();
    let r = phone::PhoneRef { dev: std::sync::Arc::new(dev.id.clone()), obj: e.obj.clone() };
    let t1 = std::time::Instant::now();
    let head = phone::read_prefix(&r, 128 * 1024).unwrap();
    println!("prefix {} bytes in {:?}", head.len(), t1.elapsed());
    let t2 = std::time::Instant::now();
    let mut all = Vec::new();
    std::io::Read::read_to_end(&mut phone::open(&r).unwrap(), &mut all).unwrap();
    println!("full {} bytes (wpd size {}) in {:?}", all.len(), e.size, t2.elapsed());
    assert_eq!(all.len() as u64, e.size);
    let m = crate::meta::read_meta_bytes(&e.name, &head, crate::fsutil::Kind::Image, std::time::UNIX_EPOCH);
    println!("meta: {:?}", m);
}

/// Real phone vs a real folder, compare only: MARKSYNC_PHONE_TARGET=D:\... cargo test --release phone_compare -- --nocapture
#[test]
fn phone_compare() {
    let Ok(target) = std::env::var("MARKSYNC_PHONE_TARGET") else { return };
    let dev = crate::phone::list_devices().remove(0);
    let ps = crate::phone::PhoneSource { device: dev.name, path: vec!["Internal shared storage".into(), "DCIM".into(), "Camera".into()] };
    let inp = sync::CompareInput {
        sources: Vec::new(),
        phones: vec![ps],
        target: PathBuf::from(target),
        kinds: KindFilter::default(),
        layout: LayoutChoice::Keep,
        scheme: Scheme::YearMonth,
        skip_hidden: true,
        org_opts: organize::OrgOptions::default(),
    };
    let p = Progress::default();
    let t0 = std::time::Instant::now();
    let items = sync::compare(&inp, &p).unwrap();
    println!("compare took {:?}", t0.elapsed());
    for st in Status::ALL {
        let v: Vec<_> = items.iter().filter(|i| i.status == st).collect();
        let bytes: u64 = v.iter().map(|i| i.size).sum();
        println!("{st:?}: {} ({} MB)", v.len(), bytes / 1_000_000);
        for i in v.iter().take(3) {
            println!("   {} -> {}", i.src.display(), i.same_as.as_ref().unwrap_or(&i.dest).display());
        }
    }
}

/// MARKSYNC_PHONE_FILE=name MARKSYNC_PHONE_TARGET=dir: phone file vs disk file, byte by byte
#[test]
fn phone_one_file() {
    let (Ok(name), Ok(dir)) = (std::env::var("MARKSYNC_PHONE_FILE"), std::env::var("MARKSYNC_PHONE_TARGET")) else { return };
    use crate::phone;
    let dev = phone::list_devices().remove(0);
    let cam = phone::resolve(&dev.id, &["Internal shared storage".into(), "DCIM".into(), "Camera".into()]).unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut hit = None;
    phone::walk(&dev.id, &cam, &cancel, &mut |_, e| if e.name == name { hit = Some(e) }).unwrap();
    let e = hit.expect("not on phone");
    let r = phone::PhoneRef { dev: std::sync::Arc::new(dev.id), obj: e.obj.clone() };
    let mut a = Vec::new();
    std::io::Read::read_to_end(&mut phone::open(&r).unwrap(), &mut a).unwrap();
    let b = std::fs::read(PathBuf::from(&dir).join(&name)).unwrap();
    println!("phone wpd size {} read {} | disk {}", e.size, a.len(), b.len());
    let first_diff = a.iter().zip(b.iter()).position(|(x, y)| x != y);
    println!("first differing byte: {first_diff:?}, equal: {}", a == b);
    let head = phone::read_prefix(&r, crate::fsutil::QUICK_CHUNK).unwrap();
    println!("quick phone {:?}", &crate::fsutil::quick_hash_bytes(e.size, &head)[..4]);
    println!("quick disk  {:?}", &crate::fsutil::quick_hash(&PathBuf::from(&dir).join(&name), b.len() as u64).unwrap()[..4]);
}

#[test]
fn albums_dates_and_dups() {
    use organize::is_meaningful_folder as m;
    for good in ["Rodjendan", "Rođendan Mila", "More Budva 2024", "Leto 2024", "Rim"] {
        assert!(m(good), "{good}");
    }
    for bad in ["Camera", "DCIM", "Nova fascikla (3)", "New folder", "2024-07", "07 - Jul", "Slike sa telefona 14 januar 2026",
                "SM-G990B_20251211193024", "WhatsApp Images", "Viber", "Filteri", "Vivid", "_Proveri datum", "Bez lokacije"] {
        assert!(!m(bad), "{bad}");
    }

    let Some(base) = fresh("albums") else { return };
    let p = Progress::default();
    let root = base.join("Laptop").join("Slike");
    let dest = base.join("Sredjeno");
    let opts = organize::OrgOptions { keep_albums: true, flag_dates: true, remove_dups: true };
    let plan = organize::plan(&root, Some(&dest), Scheme::YearMonth, OrgMode::CopyTo, KindFilter::default(), true, opts, &p).unwrap();
    for mv in &plan.moves {
        println!("{} -> {}", mv.from.strip_prefix(&root).unwrap().display(), mv.to.strip_prefix(&dest).unwrap().display());
    }
    for d in &plan.dups {
        println!("dup {} (kept {})", d.path.display(), d.kept.display());
    }
    let to = |name: &str| plan.moves.iter().find(|m| m.from.ends_with(name)).map(|m| m.to.strip_prefix(&dest).unwrap().to_string_lossy().to_string());
    assert_eq!(to("IMG_0001.jpg").unwrap(), ["2024", "07 - Jul", "Leto 2024", "IMG_0001.jpg"].join("\\"));
    assert_eq!(to("DSC_100.jpg").unwrap(), ["2023", "05 - Maj", "Rim", "DSC_100.jpg"].join("\\"));
    assert_eq!(to("IMG-20240101-WA0001.jpg").unwrap(), ["2024", "01 - Januar", "IMG-20240101-WA0001.jpg"].join("\\"), "Viber is not an album");
    assert!(to("tajna.jpg").unwrap().starts_with(r"_Proveri datum\"), "no capture date");
    assert!(to("video.mp4").unwrap().starts_with(r"_Proveri datum\"));
    assert_eq!(plan.uncertain, 2);
    assert_eq!(plan.dups.len(), 2, "slika kopija + Vivid/IMG_0002");
    assert!(plan.dups.iter().any(|d| d.path.ends_with("slika kopija.jpg") && d.kept.ends_with("slika.jpg")));
    let (rep, _) = organize::execute(&plan, &p);
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert_eq!(rep.ok_files as usize, plan.moves.len());
    assert!(root.join("Rodjendan").join("slika kopija.jpg").exists(), "copy mode leaves the source alone");
}

#[test]
fn safety_guards() {
    use crate::fsutil;
    let dir = std::env::temp_dir().join(format!("marksync-safety-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b, c) = (dir.join("a.jpg"), dir.join("b.jpg"), dir.join("c.jpg"));
    std::fs::write(&a, b"AAAA").unwrap();
    std::fs::write(&b, b"BBBB").unwrap();
    // never overwrites
    assert!(fsutil::rename_noreplace(&a, &b).is_err());
    assert_eq!(std::fs::read(&b).unwrap(), b"BBBB");
    // verified copy
    let (bytes, cancel) = (std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicBool::new(false));
    fsutil::copy_file(&a, &c, &bytes, &cancel).unwrap();
    assert_eq!(std::fs::read(&c).unwrap(), b"AAAA");
    assert!(fsutil::copy_file(&a, &c, &bytes, &cancel).is_err(), "copy must not replace an existing file");
    // a source that delivers fewer bytes than announced is rejected
    let short: Box<dyn std::io::Read> = Box::new(std::io::Cursor::new(b"AA".to_vec()));
    assert!(fsutil::copy_from(short, 4, std::time::UNIX_EPOCH, &dir.join("d.jpg"), &bytes, &cancel).is_err());
    assert!(!dir.join("d.jpg").exists());
    // duplicate removal guards
    assert!(fsutil::safe_to_remove_extra(&c, &a, &cancel).is_ok(), "real duplicate");
    assert!(fsutil::safe_to_remove_extra(&a, &a, &cancel).is_err(), "same file twice");
    assert!(fsutil::safe_to_remove_extra(&a, &b, &cancel).is_err(), "different content");
    assert!(fsutil::safe_to_remove_extra(&a, &dir.join("gone.jpg"), &cancel).is_err(), "kept file missing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn same_folder_is_refused() {
    let Some(base) = fresh("same") else { return };
    let p = Progress::default();
    let mut inp = input(&base, LayoutChoice::Keep, &[]);
    inp.target = inp.sources[0].root.clone();
    assert!(sync::compare(&inp, &p).is_err(), "source == target must be refused, not reported as 'all on disk'");
}

#[test]
fn organize_in_place_removes_dups_and_undo_works() {
    let Some(base) = fresh("inplace-dups") else { return };
    let p = Progress::default();
    let root = base.join("Laptop").join("Slike");
    let opts = organize::OrgOptions { keep_albums: true, flag_dates: true, remove_dups: true };
    let plan = organize::plan(&root, None, Scheme::YearMonth, OrgMode::MoveInPlace, KindFilter::default(), true, opts, &p).unwrap();
    assert_eq!(plan.dups.len(), 2);
    let (rep, journal) = organize::execute(&plan, &p);
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert_eq!(rep.dups_removed, 2, "extras removed although their twin was moved");
    let kept = root.join("2022").join("11 - Novembar").join("Rodjendan").join("slika.jpg");
    assert!(kept.exists(), "the kept copy is in its new place");
    let j = journal.expect("undo record");
    assert_eq!(j.extension().unwrap(), "jsonl");
    let rep = organize::undo(&j, &p);
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert!(root.join("Rodjendan").join("slika.jpg").exists());
    assert!(!j.exists(), "fully undone → record removed");
}
