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
        target: base.join("HDD"),
        kinds: KindFilter::default(),
        layout,
        scheme: Scheme::YearMonth,
        skip_hidden: true,
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
    let plan = organize::plan(&root, None, Scheme::YearMonth, OrgMode::MoveInPlace, KindFilter::default(), true, &p).unwrap();
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
