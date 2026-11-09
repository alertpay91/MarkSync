//! Translations. Serbian Cyrillic is generated from Serbian Latin.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    SrLatn,
    SrCyrl,
    En,
    De,
    Ru,
}

impl Lang {
    pub const ALL: [Lang; 5] = [Lang::SrLatn, Lang::SrCyrl, Lang::En, Lang::De, Lang::Ru];
    pub fn code(self) -> &'static str {
        match self {
            Lang::SrLatn => "srlatn",
            Lang::SrCyrl => "srcyrl",
            Lang::En => "en",
            Lang::De => "de",
            Lang::Ru => "ru",
        }
    }
    pub fn from_code(c: &str) -> Lang {
        match c.to_ascii_lowercase().as_str() {
            "srcyrl" | "sr-cyrl" | "serbiancyrillic" => Lang::SrCyrl,
            "en" | "english" => Lang::En,
            "de" | "german" => Lang::De,
            "ru" | "russian" => Lang::Ru,
            _ => Lang::SrLatn,
        }
    }
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::SrLatn => "Srpski (latinica)",
            Lang::SrCyrl => "Српски (ћирилица)",
            Lang::En => "English",
            Lang::De => "Deutsch",
            Lang::Ru => "Русский",
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(l: Lang) {
    LANG.store(Lang::ALL.iter().position(|x| *x == l).unwrap_or(0) as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    Lang::ALL[LANG.load(Ordering::Relaxed) as usize % Lang::ALL.len()]
}

fn index() -> &'static HashMap<&'static str, usize> {
    static I: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    I.get_or_init(|| S.iter().enumerate().map(|(i, r)| (r.0, i)).collect())
}

fn cyrillic() -> &'static Vec<&'static str> {
    static C: OnceLock<Vec<&'static str>> = OnceLock::new();
    C.get_or_init(|| S.iter().map(|r| &*Box::leak(to_cyrillic(r.1).into_boxed_str())).collect())
}

/// Translated text for a key (the key itself if missing).
pub fn t(key: &'static str) -> &'static str {
    let Some(&i) = index().get(key) else { return key };
    let r = &S[i];
    match lang() {
        Lang::SrLatn => r.1,
        Lang::SrCyrl => cyrillic()[i],
        Lang::En => r.2,
        Lang::De => r.3,
        Lang::Ru => r.4,
    }
}

/// Translated text with %1, %2 … replaced.
pub fn tf(key: &'static str, args: &[&str]) -> String {
    let mut s = t(key).to_string();
    for (i, a) in args.iter().enumerate().rev() {
        s = s.replace(&format!("%{}", i + 1), a);
    }
    s
}

pub fn fmt_num(n: u64) -> String {
    let sep = match lang() {
        Lang::En => ',',
        Lang::Ru => '\u{a0}',
        _ => '.',
    };
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

pub fn fmt_size(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < U.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    let s = if u == 0 { format!("{} {}", b, U[0]) } else if v >= 100.0 { format!("{:.0} {}", v, U[u]) } else { format!("{:.1} {}", v, U[u]) };
    if lang() == Lang::En { s } else { s.replace('.', ",") }
}

pub fn fmt_date(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = t.into();
    match lang() {
        Lang::En => dt.format("%d %b %Y, %H:%M").to_string(),
        Lang::SrLatn | Lang::SrCyrl => dt.format("%d.%m.%Y. %H:%M").to_string(),
        _ => dt.format("%d.%m.%Y %H:%M").to_string(),
    }
}

/// Words that stay in Latin script inside Cyrillic text.
const KEEP_LATIN: &[&str] = &[
    "_MarkSync duplikati", "MarkSync", "Claude", "Anthropic", "PayPal", "Camera", "Facebook", "Instagram", "Windows", "Android", "iPhone", "Samsung", "Apple", "Explorer", "Viber",
    "WhatsApp", "DCIM", "EXIF", "GPS", "USB", "HDD", "SSD", "SD", "TB", "GB", "MB", "KB", "OK",
];

pub fn to_cyrillic(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() * 2);
    let mut i = 0;
    'outer: while i < chars.len() {
        // protected words (only at a word start)
        if i == 0 || !chars[i - 1].is_alphanumeric() {
            for w in KEEP_LATIN {
                let wc: Vec<char> = w.chars().collect();
                if chars[i..].starts_with(&wc) && chars.get(i + wc.len()).map(|c| !c.is_alphanumeric()).unwrap_or(true) {
                    out.extend(wc.iter());
                    i += wc.len();
                    continue 'outer;
                }
            }
        }
        // %1 placeholders
        if chars[i] == '%' {
            out.push('%');
            i += 1;
            continue;
        }
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let two = match (c, next) {
            ('L', Some('j')) | ('L', Some('J')) => Some("Љ"),
            ('l', Some('j')) => Some("љ"),
            ('N', Some('j')) | ('N', Some('J')) => Some("Њ"),
            ('n', Some('j')) => Some("њ"),
            ('D', Some('ž')) | ('D', Some('Ž')) => Some("Џ"),
            ('d', Some('ž')) => Some("џ"),
            _ => None,
        };
        if let Some(t) = two {
            out.push_str(t);
            i += 2;
            continue;
        }
        let m = match c {
            'a' => "а", 'b' => "б", 'c' => "ц", 'č' => "ч", 'ć' => "ћ", 'd' => "д", 'đ' => "ђ", 'e' => "е",
            'f' => "ф", 'g' => "г", 'h' => "х", 'i' => "и", 'j' => "ј", 'k' => "к", 'l' => "л", 'm' => "м",
            'n' => "н", 'o' => "о", 'p' => "п", 'r' => "р", 's' => "с", 'š' => "ш", 't' => "т", 'u' => "у",
            'v' => "в", 'z' => "з", 'ž' => "ж", 'q' => "к", 'w' => "в", 'x' => "кс", 'y' => "ј",
            'A' => "А", 'B' => "Б", 'C' => "Ц", 'Č' => "Ч", 'Ć' => "Ћ", 'D' => "Д", 'Đ' => "Ђ", 'E' => "Е",
            'F' => "Ф", 'G' => "Г", 'H' => "Х", 'I' => "И", 'J' => "Ј", 'K' => "К", 'L' => "Л", 'M' => "М",
            'N' => "Н", 'O' => "О", 'P' => "П", 'R' => "Р", 'S' => "С", 'Š' => "Ш", 'T' => "Т", 'U' => "У",
            'V' => "В", 'Z' => "З", 'Ž' => "Ж", 'Q' => "К", 'W' => "В", 'X' => "Кс", 'Y' => "Ј",
            _ => {
                out.push(c);
                i += 1;
                continue;
            }
        };
        out.push_str(m);
        i += 1;
    }
    out
}

// (key, srpski latinica, English, Deutsch, Русский)
#[rustfmt::skip]
const S: &[(&str, &str, &str, &str, &str)] = &[
    // tabs
    ("tab_sync", "Sinhronizacija", "Sync", "Abgleich", "Синхронизация"),
    ("tab_org", "Sređivanje", "Organize", "Ordnen", "Упорядочить"),
    ("tab_dup", "Duplikati", "Duplicates", "Duplikate", "Дубликаты"),
    ("tab_settings", "Podešavanja", "Settings", "Einstellungen", "Настройки"),

    // sync setup
    ("sync_title", "Prebaci nove fajlove na disk", "Copy new files to your disk", "Neue Dateien auf die Festplatte kopieren", "Перенести новые файлы на диск"),
    ("sync_sub", "Izaberi odakle i gde. MarkSync poredi fajlove po sadržaju, pa ime fajla i folder nisu bitni — ista slika se neće kopirati dvaput.", "Choose where from and where to. MarkSync compares files by content, so names and folders don't matter — the same photo is never copied twice.", "Wähle Quelle und Ziel. MarkSync vergleicht Dateien nach Inhalt – Namen und Ordner spielen keine Rolle, dasselbe Foto wird nie doppelt kopiert.", "Выберите откуда и куда. MarkSync сравнивает файлы по содержимому, поэтому имена и папки не важны — одно и то же фото не будет скопировано дважды."),
    ("src_title", "Odakle", "From", "Von", "Откуда"),
    ("src_sub", "Folderi na računaru (ili kartica, telefon…)", "Folders on this computer (or a card, phone…)", "Ordner auf diesem Computer (oder Karte, Handy…)", "Папки на компьютере (или карта, телефон…)"),
    ("add_folder", "Dodaj folder", "Add folder", "Ordner hinzufügen", "Добавить папку"),
    ("kf_pictures", "Slike", "Pictures", "Bilder", "Изображения"),
    ("kf_videos", "Video", "Videos", "Videos", "Видео"),
    ("kf_desktop", "Radna površina", "Desktop", "Desktop", "Рабочий стол"),
    ("kf_downloads", "Preuzimanja", "Downloads", "Downloads", "Загрузки"),
    ("kf_documents", "Dokumenti", "Documents", "Dokumente", "Документы"),
    ("src_empty", "Dodaj bar jedan folder. Možeš i da ga prevučeš mišem u prozor.", "Add at least one folder. You can also drag it into the window.", "Füge mindestens einen Ordner hinzu. Du kannst ihn auch ins Fenster ziehen.", "Добавьте хотя бы одну папку. Можно просто перетащить её в окно."),
    ("src_tip", "Skini kvačicu sa foldera koje ne želiš da kopiraš. Strelica otvara podfoldere.", "Untick folders you don't want to copy. The arrow opens sub-folders.", "Entferne den Haken bei Ordnern, die nicht kopiert werden sollen. Der Pfeil öffnet Unterordner.", "Снимите галочку с папок, которые не нужно копировать. Стрелка открывает подпапки."),
    ("phone_tip", "📱 Telefon (Android, iPhone): poveži ga USB kablom, otključaj i izaberi „Prenos fajlova”, pa klikni na dugme 📱 Telefon.", "📱 Phone (Android, iPhone): connect it by USB, unlock it, choose “File transfer” and click the 📱 Phone button.", "📱 Handy (Android, iPhone): per USB verbinden, entsperren, „Dateiübertragung” wählen und auf 📱 Handy klicken.", "📱 Телефон (Android, iPhone): подключите по USB, разблокируйте, выберите «Передача файлов» и нажмите 📱 Телефон."),
    ("phone_btn", "Telefon", "Phone", "Handy", "Телефон"),
    ("phone_btn_hint", "Slike direktno sa telefona povezanog USB kablom", "Photos straight from a phone connected by USB", "Fotos direkt vom per USB verbundenen Handy", "Фото прямо с телефона, подключённого по USB"),
    ("phone_title", "Fajlovi sa telefona", "Files from a phone", "Dateien vom Handy", "Файлы с телефона"),
    ("phone_searching", "Tražim telefon…", "Looking for a phone…", "Suche Handy…", "Ищу телефон…"),
    ("phone_none", "Telefon nije pronađen.\n\n1. Poveži telefon USB kablom.\n2. Otključaj ekran telefona.\n3. Na telefonu dodirni obaveštenje „USB” i izaberi „Prenos fajlova” (File transfer).", "No phone found.\n\n1. Connect the phone with a USB cable.\n2. Unlock the phone's screen.\n3. On the phone tap the “USB” notification and choose “File transfer”.", "Kein Handy gefunden.\n\n1. Handy per USB-Kabel verbinden.\n2. Bildschirm entsperren.\n3. Auf dem Handy die „USB”-Benachrichtigung antippen und „Dateiübertragung” wählen.", "Телефон не найден.\n\n1. Подключите телефон USB-кабелем.\n2. Разблокируйте экран.\n3. На телефоне нажмите уведомление «USB» и выберите «Передача файлов»."),
    ("phone_retry", "Pokušaj ponovo", "Try again", "Erneut versuchen", "Повторить"),
    ("phone_which", "Koji telefon?", "Which phone?", "Welches Handy?", "Какой телефон?"),
    ("phone_find_camera", "Tražim folder sa slikama sa kamere…", "Looking for the camera folder…", "Suche den Kameraordner…", "Ищу папку камеры…"),
    ("phone_camera", "Slike i snimci sa kamere", "Camera photos and videos", "Kamerafotos und -videos", "Фото и видео с камеры"),
    ("phone_or_pick", "…ili izaberi drugi folder:", "…or choose another folder:", "…oder einen anderen Ordner wählen:", "…или выберите другую папку:"),
    ("phone_loading", "Učitavam… (telefon je sporiji od diska)", "Loading… (a phone is slower than a disk)", "Lade… (ein Handy ist langsamer als eine Festplatte)", "Загрузка… (телефон медленнее диска)"),
    ("phone_empty", "Ovde nema foldera.", "No folders here.", "Hier sind keine Ordner.", "Здесь нет папок."),
    ("phone_unlock_hint", "Ako je spisak prazan, otključaj telefon i proveri da je izabrano „Prenos fajlova”.", "If the list is empty, unlock the phone and check that “File transfer” is selected.", "Ist die Liste leer, entsperre das Handy und prüfe, dass „Dateiübertragung” gewählt ist.", "Если список пуст, разблокируйте телефон и проверьте, что выбрана «Передача файлов»."),
    ("phone_pick", "Izaberi", "Choose", "Wählen", "Выбрать"),
    ("phone_pick_this", "Izaberi ovaj folder", "Choose this folder", "Diesen Ordner wählen", "Выбрать эту папку"),
    ("st_scan_phone", "Čitam spisak fajlova sa telefona… (telefon je sporiji od diska)", "Reading the file list from the phone… (slower than a disk)", "Lese die Dateiliste vom Handy… (langsamer als eine Festplatte)", "Читаю список файлов с телефона… (медленнее, чем с диска)"),
    ("err_no_phone", "Telefon „%1” nije povezan. Poveži ga, otključaj i izaberi „Prenos fajlova”.", "The phone “%1” is not connected. Connect it, unlock it and choose “File transfer”.", "Das Handy „%1” ist nicht verbunden. Verbinden, entsperren und „Dateiübertragung” wählen.", "Телефон «%1» не подключён. Подключите, разблокируйте и выберите «Передача файлов»."),
    ("err_phone_folder", "Na telefonu ne mogu da nađem folder %1 (%2).", "Can't find the folder %1 on the phone (%2).", "Ordner %1 auf dem Handy nicht gefunden (%2).", "Не удаётся найти папку %1 на телефоне (%2)."),
    ("err_phone_lost", "Ne mogu da pročitam fajlove sa telefona, pa sam prekinuo poređenje (inače bi spisak bio pogrešan).\n\nProveri da je telefon i dalje povezan, otključan i da je izabrano „Prenos fajlova”, pa klikni „Uporedi” ponovo.\n\nGreška: %1", "Can't read files from the phone, so the comparison was stopped (otherwise the list would be wrong).\n\nCheck that the phone is still connected, unlocked and set to “File transfer”, then click “Compare” again.\n\nError: %1", "Dateien vom Handy können nicht gelesen werden, daher wurde der Vergleich abgebrochen (sonst wäre die Liste falsch).\n\nPrüfe, ob das Handy verbunden, entsperrt und auf „Dateiübertragung” gestellt ist, und klicke erneut auf „Vergleichen”.\n\nFehler: %1", "Не удаётся прочитать файлы с телефона, поэтому сравнение остановлено (иначе список был бы неверным).\n\nПроверьте, что телефон подключён, разблокирован и выбрана «Передача файлов», затем нажмите «Сравнить» снова.\n\nОшибка: %1"),
    ("err_phone_read", "Greška pri čitanju telefona: %1", "Error reading the phone: %1", "Fehler beim Lesen des Handys: %1", "Ошибка чтения телефона: %1"),
    ("tgt_title", "Gde", "To", "Nach", "Куда"),
    ("tgt_sub", "Eksterni disk ili folder gde čuvaš kopije", "External disk or the folder where you keep copies", "Externe Festplatte oder Ordner für die Kopien", "Внешний диск или папка для копий"),
    ("pick_folder", "Izaberi folder", "Choose folder", "Ordner wählen", "Выбрать папку"),
    ("refresh_drives", "Osveži spisak diskova", "Refresh drive list", "Laufwerke aktualisieren", "Обновить список дисков"),
    ("no_ext_drive", "Nijedan eksterni disk nije priključen. Priključi ga ili izaberi folder.", "No external disk is connected. Plug one in or choose a folder.", "Keine externe Festplatte angeschlossen. Schließe eine an oder wähle einen Ordner.", "Внешний диск не подключён. Подключите его или выберите папку."),
    ("local_disk", "Lokalni disk", "Local disk", "Lokaler Datenträger", "Локальный диск"),
    ("free_of", "slobodno %1 od %2", "%1 free of %2", "%1 frei von %2", "свободно %1 из %2"),
    ("tgt_missing", "Ovaj folder još ne postoji — biće napravljen.", "This folder doesn't exist yet — it will be created.", "Dieser Ordner existiert noch nicht – er wird angelegt.", "Эта папка ещё не существует — она будет создана."),
    ("tgt_empty", "Izaberi disk ili folder.", "Choose a disk or a folder.", "Wähle ein Laufwerk oder einen Ordner.", "Выберите диск или папку."),
    ("layout_q", "Kako da složim nove fajlove na disku:", "How to arrange new files on the disk:", "Wie neue Dateien auf dem Ziel ablegen:", "Как разложить новые файлы на диске:"),
    ("layout_keep", "Isti folderi kao na računaru", "Same folders as on the computer", "Gleiche Ordner wie auf dem Computer", "Те же папки, что на компьютере"),
    ("layout_flat", "Sve u jedan folder", "Everything in one folder", "Alles in einen Ordner", "Всё в одну папку"),
    ("layout_org", "Sredi automatski po:", "Sort automatically by:", "Automatisch ordnen nach:", "Упорядочить автоматически по:"),
    ("example", "Primer:", "Example:", "Beispiel:", "Пример:"),
    ("what_files", "Šta tražim:", "Look for:", "Suchen nach:", "Что искать:"),
    ("type_images", "Slike", "Photos", "Fotos", "Фото"),
    ("type_videos", "Snimci", "Videos", "Videos", "Видео"),
    ("type_audio", "Muzika", "Music", "Musik", "Музыка"),
    ("type_docs", "Dokumenti", "Documents", "Dokumente", "Документы"),
    ("type_other", "Ostali fajlovi", "Other files", "Andere Dateien", "Другие файлы"),
    ("compare", "Uporedi", "Compare", "Vergleichen", "Сравнить"),
    ("compare_need", "Dodaj folder levo, izaberi disk desno i bar jednu vrstu fajlova.", "Add a folder on the left, pick a disk on the right and at least one file type.", "Links einen Ordner hinzufügen, rechts ein Ziel wählen und mindestens einen Dateityp.", "Добавьте папку слева, выберите диск справа и хотя бы один тип файлов."),
    ("safe_note", "🔒 Ništa se ne kopira ni briše dok ne pogledaš spisak i potvrdiš.", "🔒 Nothing is copied or deleted until you review the list and confirm.", "🔒 Nichts wird kopiert oder gelöscht, bevor du die Liste geprüft und bestätigt hast.", "🔒 Ничего не копируется и не удаляется, пока вы не посмотрите список и не подтвердите."),
    ("err_same_folder", "Izvor i odredište ne mogu biti isti folder.", "Source and destination can't be the same folder.", "Quelle und Ziel dürfen nicht derselbe Ordner sein.", "Источник и назначение не могут совпадать."),
    ("remove", "Ukloni", "Remove", "Entfernen", "Убрать"),
    ("no_subfolders", "nema podfoldera", "no sub-folders", "keine Unterordner", "нет подпапок"),

    // sync results
    ("results_title", "Rezultat poređenja", "Comparison result", "Ergebnis des Vergleichs", "Результат сравнения"),
    ("back", "Nazad", "Back", "Zurück", "Назад"),
    ("compare_again", "Uporedi ponovo", "Compare again", "Erneut vergleichen", "Сравнить снова"),
    ("click_to_toggle", "Klikni da prikažeš ili sakriješ.", "Click to show or hide.", "Klicken zum Ein-/Ausblenden.", "Нажмите, чтобы показать или скрыть."),
    ("st_new", "Novo", "New", "Neu", "Новое"),
    ("st_newer", "Novija verzija", "Newer version", "Neuere Version", "Новая версия"),
    ("st_conflict", "Isto ime", "Same name", "Gleicher Name", "То же имя"),
    ("st_exists", "Već je na disku", "Already on disk", "Schon vorhanden", "Уже на диске"),
    ("st_dup_src", "Duplikat u izvoru", "Duplicate in source", "Duplikat in Quelle", "Дубликат в источнике"),
    ("st_new_hint", "Ovog fajla nema na disku — ni pod drugim imenom ni u drugom folderu.", "This file is not on the disk — not even under another name or folder.", "Diese Datei ist nicht auf dem Ziel – auch nicht unter anderem Namen oder Ordner.", "Этого файла нет на диске — ни под другим именем, ни в другой папке."),
    ("st_newer_hint", "Na disku postoji fajl sa istim imenom na istom mestu, ali je ovaj na računaru noviji (izmenjen).", "A file with the same name is in the same place on the disk, but the one on the computer is newer (edited).", "Auf dem Ziel liegt eine gleichnamige Datei, aber die auf dem Computer ist neuer (bearbeitet).", "На диске есть файл с тем же именем, но файл на компьютере новее (изменён)."),
    ("st_conflict_hint", "Na disku postoji drugačiji fajl sa istim imenom. Biraš da li da sačuvaš oba ili da zameniš.", "A different file with the same name is on the disk. Choose whether to keep both or replace.", "Auf dem Ziel liegt eine andere Datei mit gleichem Namen. Wähle: beide behalten oder ersetzen.", "На диске есть другой файл с тем же именем. Выберите: сохранить оба или заменить."),
    ("st_exists_hint", "Potpuno isti fajl (isti sadržaj) već postoji na disku, možda pod drugim imenom ili u drugom folderu. Neće se kopirati.", "The exact same file (same content) is already on the disk, maybe with another name or folder. It won't be copied.", "Exakt dieselbe Datei (gleicher Inhalt) ist schon auf dem Ziel, evtl. unter anderem Namen. Sie wird nicht kopiert.", "Точно такой же файл (то же содержимое) уже есть на диске, возможно под другим именем. Копироваться не будет."),
    ("st_dup_src_hint", "Ovaj fajl se na računaru nalazi više puta. Kopira se samo jedan primerak.", "This file appears several times on the computer. Only one copy is transferred.", "Diese Datei liegt mehrfach auf dem Computer. Nur ein Exemplar wird kopiert.", "Этот файл встречается на компьютере несколько раз. Копируется только один экземпляр."),
    ("nothing_found", "Nisam našao nijedan fajl izabrane vrste.", "No files of the chosen type were found.", "Keine Dateien des gewählten Typs gefunden.", "Файлы выбранного типа не найдены."),
    ("all_there", "Sve je već na disku. Nema šta da se kopira.", "Everything is already on the disk. Nothing to copy.", "Alles ist schon auf dem Ziel. Nichts zu kopieren.", "Всё уже на диске. Копировать нечего."),
    ("search", "Traži po imenu…", "Search by name…", "Nach Namen suchen…", "Поиск по имени…"),
    ("select_all", "Štikliraj sve", "Select all", "Alle auswählen", "Выбрать все"),
    ("select_none", "Odštikliraj sve", "Select none", "Keine auswählen", "Снять выбор"),
    ("shown", "Prikazano: %1", "Shown: %1", "Angezeigt: %1", "Показано: %1"),
    ("col_file", "Fajl", "File", "Datei", "Файл"),
    ("col_size", "Veličina", "Size", "Größe", "Размер"),
    ("col_date", "Datum", "Date", "Datum", "Дата"),
    ("col_status", "Stanje", "Status", "Status", "Статус"),
    ("col_action", "Šta da radim", "Action", "Aktion", "Действие"),
    ("act_copy", "Kopiraj", "Copy", "Kopieren", "Копировать"),
    ("act_copy_renamed", "Kopiraj (novo ime)", "Copy (new name)", "Kopieren (neuer Name)", "Копировать (новое имя)"),
    ("act_replace", "Zameni stari", "Replace old one", "Alte ersetzen", "Заменить старый"),
    ("act_keep_both", "Sačuvaj oba", "Keep both", "Beide behalten", "Сохранить оба"),
    ("on_disk_date", "Fajl na disku: %1", "File on disk: %1", "Datei auf dem Ziel: %1", "Файл на диске: %1"),
    ("show_in_explorer", "Prikaži u Exploreru", "Show in Explorer", "Im Explorer zeigen", "Показать в Проводнике"),
    ("selected_sum", "Izabrano: %1 fajlova · %2", "Selected: %1 files · %2", "Ausgewählt: %1 Dateien · %2", "Выбрано: %1 файлов · %2"),
    ("free_on_target", "Slobodno na disku: %1", "Free on disk: %1", "Frei auf dem Ziel: %1", "Свободно на диске: %1"),
    ("not_enough", "nema dovoljno mesta!", "not enough space!", "nicht genug Platz!", "недостаточно места!"),
    ("copy_selected", "Kopiraj izabrano", "Copy selected", "Auswahl kopieren", "Копировать выбранное"),
    ("old_version", "stara verzija", "old version", "alte Version", "старая версия"),

    // organize
    ("org_title", "Sredi folder", "Organize a folder", "Ordner aufräumen", "Упорядочить папку"),
    ("org_sub", "MarkSync poređa slike i snimke u foldere po datumu, mestu, slovu ili telefonu kojim su snimljeni.", "MarkSync sorts photos and videos into folders by date, place, letter or the phone that took them.", "MarkSync sortiert Fotos und Videos in Ordner nach Datum, Ort, Buchstabe oder Kamera.", "MarkSync разложит фото и видео по папкам: по дате, месту, букве или камере."),
    ("org_folder", "Folder koji sređuješ", "Folder to organize", "Ordner zum Aufräumen", "Папка для упорядочивания"),
    ("org_folder_sub", "Na primer ceo eksterni disk ili folder sa slikama.", "For example the whole external disk or a photo folder.", "Zum Beispiel die ganze externe Festplatte oder ein Fotoordner.", "Например, весь внешний диск или папка с фото."),
    ("use_sync_target", "Isti kao u sinhronizaciji", "Same as in Sync", "Wie beim Abgleich", "Как в синхронизации"),
    ("org_how", "Kako da sredim", "How to organize", "Wie ordnen", "Как упорядочить"),
    ("org_how_sub", "Izaberi raspored foldera.", "Choose the folder layout.", "Wähle die Ordnerstruktur.", "Выберите структуру папок."),
    ("sch_ym", "Godina i mesec", "Year and month", "Jahr und Monat", "Год и месяц"),
    ("sch_ymd", "Godina, mesec i dan", "Year, month and day", "Jahr, Monat und Tag", "Год, месяц и день"),
    ("sch_y", "Samo godina", "Year only", "Nur Jahr", "Только год"),
    ("sch_abc", "Po abecedi (ABC)", "Alphabetical (ABC)", "Alphabetisch (ABC)", "По алфавиту (ABC)"),
    ("sch_loc", "Po mestu (država / grad)", "By place (country / city)", "Nach Ort (Land / Stadt)", "По месту (страна / город)"),
    ("sch_yloc", "Godina pa mesto (putovanja)", "Year, then place (trips)", "Jahr, dann Ort (Reisen)", "Год, затем место (поездки)"),
    ("sch_type", "Po vrsti fajla", "By file type", "Nach Dateityp", "По типу файла"),
    ("sch_cam", "Po telefonu / aparatu", "By phone / camera", "Nach Handy / Kamera", "По телефону / камере"),
    ("loc_note", "Mesto se čita iz GPS podataka u slici (telefoni ga obično upisuju). Radi bez interneta. Slike bez lokacije idu u folder „Bez lokacije”.", "The place is read from the photo's GPS data (phones usually store it). Works offline. Photos without location go to “No location”.", "Der Ort wird aus den GPS-Daten des Fotos gelesen (Handys speichern sie meist). Funktioniert offline. Fotos ohne Ort kommen nach „Ohne Ort”.", "Место берётся из GPS-данных фото (телефоны обычно их записывают). Работает без интернета. Фото без места попадут в «Без места»."),
    ("org_mode", "Način", "Mode", "Modus", "Режим"),
    ("org_move", "Premesti fajlove u nove foldere (unutar istog foldera)", "Move files into new folders (inside the same folder)", "Dateien in neue Ordner verschieben (im selben Ordner)", "Переместить файлы в новые папки (внутри той же папки)"),
    ("org_move_hint", "Brzo i ne zauzima dodatni prostor. Može da se poništi jednim klikom.", "Fast and takes no extra space. Can be undone with one click.", "Schnell, braucht keinen zusätzlichen Platz. Mit einem Klick rückgängig zu machen.", "Быстро и не занимает места. Можно отменить одним нажатием."),
    ("org_copy", "Kopiraj sređeno u drugi folder", "Copy, organized, into another folder", "Geordnet in einen anderen Ordner kopieren", "Скопировать упорядоченно в другую папку"),
    ("org_copy_hint", "Original ostaje netaknut, ali treba duplo više mesta.", "The original stays untouched, but it needs twice the space.", "Das Original bleibt unverändert, braucht aber doppelten Platz.", "Оригинал не меняется, но нужно вдвое больше места."),
    ("err_short_read", "Telefon/disk je poslao manje podataka nego što fajl ima — kopija je obrisana.", "The phone/disk sent less data than the file has — the copy was deleted.", "Handy/Laufwerk hat weniger Daten geliefert als die Datei hat – Kopie gelöscht.", "Телефон/диск передал меньше данных, чем в файле, — копия удалена."),
    ("err_verify", "Provera kopije nije uspela (sadržaj se razlikuje) — kopija je obrisana, original je netaknut.", "Copy check failed (content differs) — the copy was deleted, the original is untouched.", "Prüfung der Kopie fehlgeschlagen (Inhalt abweichend) – Kopie gelöscht, Original unverändert.", "Проверка копии не прошла (содержимое отличается) — копия удалена, оригинал не тронут."),
    ("err_kept_missing", "Preskočeno: fajl koji se zadržava više ne postoji.", "Skipped: the file that is kept no longer exists.", "Übersprungen: die behaltene Datei existiert nicht mehr.", "Пропущено: сохраняемый файл больше не существует."),
    ("err_not_dup", "Preskočeno: fajlovi više nisu potpuno isti.", "Skipped: the files are no longer identical.", "Übersprungen: die Dateien sind nicht mehr identisch.", "Пропущено: файлы больше не одинаковые."),
    ("history", "Istorija (šta je program radio)", "History (what the program did)", "Verlauf (was das Programm getan hat)", "История (что делала программа)"),
    ("history_hint", "Svako kopiranje, premeštanje i uklanjanje je zapisano: odakle i gde.", "Every copy, move and removal is written down: from where and to where.", "Jedes Kopieren, Verschieben und Entfernen wird notiert: woher und wohin.", "Каждое копирование, перемещение и удаление записано: откуда и куда."),
    ("log_copied", "KOPIRANO", "COPIED", "KOPIERT", "СКОПИРОВАНО"),
    ("log_moved", "PREMEŠTENO", "MOVED", "VERSCHOBEN", "ПЕРЕМЕЩЕНО"),
    ("log_undone", "VRAĆENO", "MOVED BACK", "ZURÜCK", "ВОЗВРАЩЕНО"),
    ("log_to_bin", "U KORPU (stara verzija)", "TO RECYCLE BIN (old version)", "IN PAPIERKORB (alte Version)", "В КОРЗИНУ (старая версия)"),
    ("log_renamed", "PREIMENOVANO (stara verzija)", "RENAMED (old version)", "UMBENANNT (alte Version)", "ПЕРЕИМЕНОВАНО (старая версия)"),
    ("log_dup_bin", "DUPLIKAT U KORPU (zadržan je)", "DUPLICATE TO RECYCLE BIN (kept:)", "DUPLIKAT IN PAPIERKORB (behalten:)", "ДУБЛИКАТ В КОРЗИНУ (оставлен:)"),
    ("after_copy_q", "Da sredim i fajlove koji su već bili na disku?", "Organize the files that were already on the disk too?", "Auch die Dateien ordnen, die schon auf dem Ziel waren?", "Упорядочить и файлы, которые уже были на диске?"),
    ("after_copy_hint", "Novi fajlovi su složeni po izabranom rasporedu, a stari su ostali gde su bili. Ovo ih poređa u iste foldere — prvo vidiš pregled, a može i da se poništi.", "The new files are arranged, the old ones stayed where they were. This puts them into the same folders — you see a preview first, and it can be undone.", "Neue Dateien sind geordnet, alte blieben, wo sie waren. Das sortiert sie in dieselben Ordner – erst mit Vorschau, rückgängig möglich.", "Новые файлы разложены, старые остались на месте. Это разложит их по тем же папкам — сначала предпросмотр, можно отменить."),
    ("after_copy_btn", "Sredi i stare fajlove po istom rasporedu", "Organize the old files the same way", "Alte Dateien genauso ordnen", "Упорядочить старые файлы так же"),
    ("warn_unreadable", "%1 fajlova/foldera nije moglo da se pročita — oni NISU u ovom spisku.", "%1 files/folders could not be read — they are NOT in this list.", "%1 Dateien/Ordner konnten nicht gelesen werden – sie sind NICHT in dieser Liste.", "%1 файлов/папок не удалось прочитать — их НЕТ в этом списке."),
    ("warn_unreadable_hint", "Ne briši ništa sa izvora dok ovo ne proveriš (npr. zaključan folder, fajl u upotrebi, telefon se zaključao).", "Don't delete anything from the source until you've checked these (e.g. a locked folder, a file in use, the phone locked).", "Lösche nichts in der Quelle, bevor du das geprüft hast (z. B. gesperrter Ordner, Datei in Benutzung, Handy gesperrt).", "Ничего не удаляйте из источника, пока не проверите это (например, закрытая папка, файл занят, телефон заблокирован)."),
    ("warn_show", "Prikaži spisak", "Show the list", "Liste zeigen", "Показать список"),
    ("warn_phone_skipped", "%1 objekata na telefonu nije moglo da se pročita (%2)", "%1 objects on the phone could not be read (%2)", "%1 Objekte auf dem Handy nicht lesbar (%2)", "%1 объектов на телефоне не удалось прочитать (%2)"),
    ("err_phone_changed", "Fajl na telefonu više nije isti kao pri poređenju (telefon je ponovo povezan?) — klikni „Uporedi ponovo”.", "The file on the phone is no longer the one compared (phone reconnected?) — click “Compare again”.", "Die Datei auf dem Handy ist nicht mehr dieselbe (neu verbunden?) – „Erneut vergleichen” klicken.", "Файл на телефоне уже не тот, что при сравнении (телефон переподключён?) — нажмите «Сравнить снова»."),
    ("org_extra", "Dodatno", "Extras", "Extras", "Дополнительно"),
    ("opt_albums", "Zadrži imena foldera kao albume", "Keep folder names as albums", "Ordnernamen als Alben behalten", "Сохранять имена папок как альбомы"),
    ("opt_albums_hint", "Npr. 2024\\07 - Jul\\Rođendan Mila. Folderi kao „Nova fascikla”, „Camera”, „DCIM” ili nazivi sa datumom se preskaču.", "E.g. 2024\\07 - July\\Mila's birthday. Folders like “New folder”, “Camera”, “DCIM” or names with dates are skipped.", "Z. B. 2024\\07 - Juli\\Milas Geburtstag. Ordner wie „Neuer Ordner”, „Camera”, „DCIM” oder Namen mit Datum werden übersprungen.", "Напр. 2024\\07 - Июль\\День рождения Милы. Папки вроде «Новая папка», «Camera», «DCIM» или названия с датами пропускаются."),
    ("opt_check_date", "Slike sa nesigurnim datumom stavi u „_Proveri datum”", "Put photos with an uncertain date in “_Check date”", "Fotos mit unsicherem Datum nach „_Datum prüfen”", "Фото с неточной датой — в «_Проверить дату»"),
    ("opt_check_date_hint", "Za slike bez datuma snimanja (npr. skinute sa Facebooka) zna se samo kad je fajl poslednji put menjan — to je često pogrešno.", "Photos without a capture date (e.g. downloaded from Facebook) only have the file's change date, which is often wrong.", "Bei Fotos ohne Aufnahmedatum (z. B. von Facebook) ist nur das Änderungsdatum bekannt – oft falsch.", "У фото без даты съёмки (например, из Facebook) известна только дата изменения файла — часто неверная."),
    ("opt_dups", "Ukloni duplikate dok sređuješ", "Remove duplicates while organizing", "Duplikate beim Ordnen entfernen", "Удалять дубликаты при упорядочивании"),
    ("opt_dups_hint", "Ista slika (isti sadržaj) se premešta samo jednom; višak ide u Korpu.", "The same photo (same content) is moved only once; extra copies go to the Recycle Bin.", "Dasselbe Foto (gleicher Inhalt) wird nur einmal verschoben; Kopien kommen in den Papierkorb.", "Одно и то же фото (то же содержимое) перемещается один раз; лишние копии — в Корзину."),
    ("check_date", "Proveri datum", "Check date", "Datum prüfen", "Проверить дату"),
    ("org_dups", "Duplikata: %1 (%2)", "Duplicates: %1 (%2)", "Duplikate: %1 (%2)", "Дубликатов: %1 (%2)"),
    ("org_uncertain", "Nesiguran datum: %1", "Uncertain date: %1", "Unsicheres Datum: %1", "Неточная дата: %1"),
    ("confirm_org_dups", "Još %1 duplikata (iste slike na više mesta) ide u Korpu — odatle mogu da se vrate.", "Another %1 duplicates (same photo in several places) go to the Recycle Bin — they can be restored from there.", "Weitere %1 Duplikate (gleiches Foto an mehreren Orten) kommen in den Papierkorb – von dort wiederherstellbar.", "Ещё %1 дубликатов (одно фото в нескольких местах) отправятся в Корзину — их можно восстановить."),
    ("confirm_org_dups_copy", "%1 duplikata se neće kopirati (kopira se samo jedan primerak).", "%1 duplicates won't be copied (only one copy is).", "%1 Duplikate werden nicht kopiert (nur ein Exemplar).", "%1 дубликатов не будут скопированы (копируется один экземпляр)."),
    ("rep_dups_removed", "Duplikata uklonjeno: %1", "Duplicates removed: %1", "Duplikate entfernt: %1", "Удалено дубликатов: %1"),
    ("org_preview","Prikaži pregled", "Show preview", "Vorschau zeigen", "Показать предпросмотр"),
    ("org_preview_title", "Pregled sređivanja", "Organize preview", "Vorschau", "Предпросмотр"),
    ("org_will_move", "Biće premešteno: %1 (%2)", "Will be moved: %1 (%2)", "Wird verschoben: %1 (%2)", "Будет перемещено: %1 (%2)"),
    ("org_will_copy", "Biće kopirano: %1 (%2)", "Will be copied: %1 (%2)", "Wird kopiert: %1 (%2)", "Будет скопировано: %1 (%2)"),
    ("org_in_place", "Već na mestu: %1", "Already in place: %1", "Schon am richtigen Ort: %1", "Уже на месте: %1"),
    ("org_folders", "Foldera: %1", "Folders: %1", "Ordner: %1", "Папок: %1"),
    ("org_new_folders", "Folderi posle sređivanja", "Folders after organizing", "Ordner nach dem Ordnen", "Папки после упорядочивания"),
    ("org_examples", "Šta gde ide", "What goes where", "Was wohin kommt", "Что куда"),
    ("org_do", "Sredi", "Organize", "Ordnen", "Упорядочить"),
    ("org_nothing", "Sve je već sređeno.", "Everything is already organized.", "Alles ist schon geordnet.", "Всё уже упорядочено."),
    ("undo_title", "Poništi poslednje sređivanje", "Undo the last organize", "Letztes Ordnen rückgängig machen", "Отменить последнее упорядочивание"),
    ("undo_info", "%1 · fajlova: %2 · %3", "%1 · files: %2 · %3", "%1 · Dateien: %2 · %3", "%1 · файлов: %2 · %3"),
    ("undo", "Poništi", "Undo", "Rückgängig", "Отменить"),
    ("no_date", "Bez datuma", "No date", "Ohne Datum", "Без даты"),
    ("no_location", "Bez lokacije", "No location", "Ohne Ort", "Без места"),
    ("no_camera", "Nepoznat uređaj", "Unknown device", "Unbekanntes Gerät", "Неизвестное устройство"),
    ("m1", "Januar", "January", "Januar", "Январь"),
    ("m2", "Februar", "February", "Februar", "Февраль"),
    ("m3", "Mart", "March", "März", "Март"),
    ("m4", "April", "April", "April", "Апрель"),
    ("m5", "Maj", "May", "Mai", "Май"),
    ("m6", "Jun", "June", "Juni", "Июнь"),
    ("m7", "Jul", "July", "Juli", "Июль"),
    ("m8", "Avgust", "August", "August", "Август"),
    ("m9", "Septembar", "September", "September", "Сентябрь"),
    ("m10", "Oktobar", "October", "Oktober", "Октябрь"),
    ("m11", "Novembar", "November", "November", "Ноябрь"),
    ("m12", "Decembar", "December", "Dezember", "Декабрь"),

    // duplicates
    ("dup_title", "Nađi duplikate", "Find duplicates", "Duplikate finden", "Найти дубликаты"),
    ("dup_sub", "Pronađi iste fajlove koji zauzimaju mesto više puta — na računaru ili na disku.", "Find identical files that take up space more than once — on the computer or the disk.", "Finde identische Dateien, die mehrfach Platz belegen – auf dem Computer oder dem Laufwerk.", "Найдите одинаковые файлы, которые занимают место несколько раз — на компьютере или на диске."),
    ("dup_where", "Gde da tražim", "Where to look", "Wo suchen", "Где искать"),
    ("dup_where_sub", "Možeš dodati više foldera ili ceo disk.", "You can add several folders or a whole disk.", "Du kannst mehrere Ordner oder ein ganzes Laufwerk hinzufügen.", "Можно добавить несколько папок или весь диск."),
    ("dup_find", "Traži duplikate", "Find duplicates", "Duplikate suchen", "Искать дубликаты"),
    ("dup_note", "Duplikat znači potpuno isti sadržaj, bajt po bajt — ime fajla nije bitno.", "A duplicate means exactly the same content, byte by byte — the file name doesn't matter.", "Duplikat heißt exakt gleicher Inhalt, Byte für Byte – der Name spielt keine Rolle.", "Дубликат — это точно такое же содержимое, байт в байт; имя файла не важно."),
    ("dup_results", "Pronađeni duplikati", "Duplicates found", "Gefundene Duplikate", "Найденные дубликаты"),
    ("dup_none", "Nema duplikata.", "No duplicates.", "Keine Duplikate.", "Дубликатов нет."),
    ("dup_groups", "Grupa: %1", "Groups: %1", "Gruppen: %1", "Групп: %1"),
    ("dup_extra", "Viška fajlova: %1", "Extra files: %1", "Überzählige Dateien: %1", "Лишних файлов: %1"),
    ("dup_wasted", "Oslobodiće se: %1", "Space to free: %1", "Freizugebender Platz: %1", "Освободится: %1"),
    ("dup_keep_hint", "U svakoj grupi zadržava se jedan fajl (zeleni). Klikni na kružić da izabereš koji. Ostali se uklanjaju.", "One file per group is kept (green). Click a circle to choose which. The others are removed.", "Pro Gruppe bleibt eine Datei (grün). Klicke auf den Kreis, um sie zu wählen. Die anderen werden entfernt.", "В каждой группе остаётся один файл (зелёный). Нажмите на кружок, чтобы выбрать. Остальные будут удалены."),
    ("dup_group_head", "%1 iste kopije · %2 svaka", "%1 identical copies · %2 each", "%1 gleiche Kopien · je %2", "%1 одинаковых копий · по %2"),
    ("dup_keep", "Zadržava se", "Kept", "Bleibt", "Остаётся"),
    ("dup_what_extra", "Šta sa viškom:", "Extra copies go to:", "Überzählige Kopien:", "Лишние копии:"),
    ("dup_to_bin", "Pošalji u Korpu (može da se vrati)", "Recycle Bin (can be restored)", "Papierkorb (wiederherstellbar)", "В Корзину (можно восстановить)"),
    ("dup_to_folder", "Premesti u folder „_MarkSync duplikati”", "Move to the “_MarkSync duplikati” folder", "In den Ordner „_MarkSync duplikati” verschieben", "Переместить в папку «_MarkSync duplikati»"),
    ("dup_remove", "Ukloni višak (%1 · %2)", "Remove extras (%1 · %2)", "Überzählige entfernen (%1 · %2)", "Убрать лишнее (%1 · %2)"),

    // settings
    ("language", "Jezik", "Language", "Sprache", "Язык"),
    ("theme", "Izgled", "Appearance", "Darstellung", "Оформление"),
    ("theme_dark", "Tamna tema", "Dark", "Dunkel", "Тёмная"),
    ("theme_light", "Svetla tema", "Light", "Hell", "Светлая"),
    ("advanced", "Napredno", "Advanced", "Erweitert", "Дополнительно"),
    ("skip_hidden", "Preskači skrivene i sistemske fajlove", "Skip hidden and system files", "Versteckte und Systemdateien überspringen", "Пропускать скрытые и системные файлы"),
    ("clear_cache", "Obriši zapamćene otiske fajlova", "Clear remembered file fingerprints", "Gespeicherte Fingerabdrücke löschen", "Очистить сохранённые отпечатки файлов"),
    ("clear_cache_hint", "Otisci ubrzavaju ponovno poređenje. Brisanje je bezbedno.", "Fingerprints speed up the next comparison. Clearing is safe.", "Fingerabdrücke beschleunigen den nächsten Vergleich. Löschen ist unbedenklich.", "Отпечатки ускоряют повторное сравнение. Очистка безопасна."),
    ("cache_cleared", "Otisci su obrisani.", "Fingerprints cleared.", "Fingerabdrücke gelöscht.", "Отпечатки очищены."),
    ("open_data", "Otvori folder sa podešavanjima", "Open the settings folder", "Einstellungsordner öffnen", "Открыть папку настроек"),
    ("about", "O programu", "About", "Über", "О программе"),
    ("about_text", "MarkSync poredi fajlove po sadržaju (otisak BLAKE3), kopira samo ono što nedostaje i nikad ništa ne briše bez pitanja. Stari fajlovi koji se zamenjuju idu u Korpu. Lokacije slika se određuju bez interneta (GeoNames).", "MarkSync compares files by content (BLAKE3 fingerprint), copies only what's missing and never deletes anything without asking. Replaced files go to the Recycle Bin. Photo places are found offline (GeoNames).", "MarkSync vergleicht Dateien nach Inhalt (BLAKE3-Fingerabdruck), kopiert nur Fehlendes und löscht nie etwas ohne Nachfrage. Ersetzte Dateien landen im Papierkorb. Fotoorte werden offline bestimmt (GeoNames).", "MarkSync сравнивает файлы по содержимому (отпечаток BLAKE3), копирует только недостающее и никогда ничего не удаляет без спроса. Заменённые файлы попадают в Корзину. Места на фото определяются без интернета (GeoNames)."),

    ("coffee_title", "Časti kafom", "Buy me a coffee", "Spendier mir einen Kaffee", "Угостить кофе"),
    ("coffee_text", "Ako ti je MarkSync pomogao da središ slike, mala zahvalnost mi pomaže da ga i dalje poboljšavam. Hvala!", "If MarkSync helped you sort your photos, a small thank-you helps me keep improving it. Thank you!", "Wenn MarkSync dir beim Ordnen geholfen hat, hilft mir ein kleines Dankeschön, es weiter zu verbessern. Danke!", "Если MarkSync помог навести порядок в фото, небольшая благодарность поможет мне его улучшать. Спасибо!"),
    ("coffee_hover", "Otvara PayPal u pregledaču", "Opens PayPal in your browser", "Öffnet PayPal im Browser", "Откроет PayPal в браузере"),
    ("copy", "Kopiraj", "Copy", "Kopieren", "Копировать"),
    ("copied", "Kopirano.", "Copied.", "Kopiert.", "Скопировано."),
    ("made_by", "Napravio: %1 — besplatno za korišćenje.", "Made by %1 — free to use.", "Erstellt von %1 – kostenlos.", "Автор: %1 — бесплатно."),
    ("made_with_claude", "Napravljeno uz pomoć Claude-a, AI asistenta kompanije Anthropic.", "Built together with Claude, an AI assistant made by Anthropic.", "Gemeinsam mit Claude entwickelt, einem KI-Assistenten von Anthropic.", "Создано вместе с Claude — ИИ-ассистентом от Anthropic."),

    // jobs
    ("job_compare", "Poređenje", "Comparing", "Vergleich", "Сравнение"),
    ("job_copy", "Kopiranje", "Copying", "Kopieren", "Копирование"),
    ("job_plan", "Priprema sređivanja", "Preparing", "Vorbereitung", "Подготовка"),
    ("job_org", "Sređivanje", "Organizing", "Ordnen", "Упорядочивание"),
    ("job_undo", "Poništavanje", "Undoing", "Rückgängig machen", "Отмена"),
    ("job_dupes", "Traženje duplikata", "Finding duplicates", "Duplikatsuche", "Поиск дубликатов"),
    ("job_dup_remove", "Uklanjanje viška", "Removing extras", "Entfernen", "Удаление лишнего"),
    ("st_scan_src", "Pregledam foldere na računaru…", "Scanning folders on the computer…", "Durchsuche Ordner auf dem Computer…", "Просматриваю папки на компьютере…"),
    ("st_scan_tgt", "Pregledam disk…", "Scanning the disk…", "Durchsuche das Ziel…", "Просматриваю диск…"),
    ("st_scanning", "Pregledam foldere…", "Scanning folders…", "Durchsuche Ordner…", "Просматриваю папки…"),
    ("st_quick_hash", "Brzo upoređujem fajlove iste veličine…", "Quick check of same-size files…", "Schnellprüfung gleich großer Dateien…", "Быстрая проверка файлов одного размера…"),
    ("st_full_hash", "Proveravam sadržaj mogućih duplikata…", "Checking the content of possible duplicates…", "Prüfe Inhalt möglicher Duplikate…", "Проверяю содержимое возможных дубликатов…"),
    ("st_classify", "Pravim spisak…", "Building the list…", "Erstelle die Liste…", "Составляю список…"),
    ("st_reading_meta", "Čitam datume i lokacije iz slika…", "Reading dates and places from photos…", "Lese Datum und Ort aus den Fotos…", "Читаю даты и места из фото…"),
    ("st_copying", "Kopiram…", "Copying…", "Kopiere…", "Копирую…"),
    ("st_organizing", "Sređujem…", "Organizing…", "Ordne…", "Упорядочиваю…"),
    ("st_undoing", "Vraćam fajlove na staro mesto…", "Moving files back…", "Verschiebe Dateien zurück…", "Возвращаю файлы на место…"),
    ("st_removing", "Uklanjam višak…", "Removing extras…", "Entferne Überzähliges…", "Убираю лишнее…"),
    ("full_detail", "Upoređujem %1 fajlova sa izvora (%2) sa %3 fajlova iste veličine na disku (%4). Čitaju se obe strane do kraja — zato je ukupno više nego što imaš slika. Sledeći put ide brzo: otisci se pamte.", "Comparing %1 source files (%2) with %3 same-size files on the disk (%4). Both sides are read in full — that's why the total is larger than your photos. Next time is fast: fingerprints are remembered.", "Vergleiche %1 Quelldateien (%2) mit %3 gleich großen Dateien auf dem Ziel (%4). Beide Seiten werden ganz gelesen – daher die größere Summe. Nächstes Mal geht es schnell: Fingerabdrücke werden gespeichert.", "Сравниваю %1 файлов источника (%2) с %3 файлами того же размера на диске (%4). Читаются обе стороны целиком — поэтому сумма больше ваших фото. В следующий раз быстрее: отпечатки запоминаются."),
    ("files_done", "Fajlova: %1 od %2", "Files: %1 of %2", "Dateien: %1 von %2", "Файлов: %1 из %2"),
    ("read_done", "pročitano %1 od %2", "read %1 of %2", "gelesen %1 von %2", "прочитано %1 из %2"),
    ("found_n", "Pronađeno: %1", "Found: %1", "Gefunden: %1", "Найдено: %1"),
    ("cancel", "Otkaži", "Cancel", "Abbrechen", "Отмена"),
    ("cancelling", "Prekidam…", "Cancelling…", "Breche ab…", "Прерываю…"),
    ("cancelled", "Prekinuto.", "Cancelled.", "Abgebrochen.", "Прервано."),

    // dialogs
    ("confirm_title", "Da li si siguran?", "Are you sure?", "Bist du sicher?", "Вы уверены?"),
    ("yes_go", "Da, kreni", "Yes, go", "Ja, los", "Да, начать"),
    ("confirm_copy", "Kopiraću %1 fajlova (%2) na:\n%3", "I'll copy %1 files (%2) to:\n%3", "Ich kopiere %1 Dateien (%2) nach:\n%3", "Скопирую %1 файлов (%2) в:\n%3"),
    ("confirm_replace", "Stara verzija %1 fajlova biće zamenjena novom. Stara ide u Korpu (ili dobija oznaku „stara verzija” ako disk nema Korpu).", "The old version of %1 files will be replaced. The old one goes to the Recycle Bin (or is renamed “old version” if the disk has none).", "Bei %1 Dateien wird die alte Version ersetzt. Die alte kommt in den Papierkorb (oder wird als „alte Version” umbenannt).", "Старая версия %1 файлов будет заменена. Старая попадёт в Корзину (или получит пометку «старая версия», если Корзины нет)."),
    ("confirm_org_move", "Premestiću %1 fajlova u nove foldere. Ovo možeš poništiti kasnije.", "I'll move %1 files into new folders. You can undo this later.", "Ich verschiebe %1 Dateien in neue Ordner. Du kannst das später rückgängig machen.", "Перемещу %1 файлов в новые папки. Позже это можно отменить."),
    ("confirm_org_copy", "Kopiraću %1 fajlova u sređene foldere. Original ostaje netaknut.", "I'll copy %1 files into organized folders. The original stays untouched.", "Ich kopiere %1 Dateien in geordnete Ordner. Das Original bleibt unverändert.", "Скопирую %1 файлов в упорядоченные папки. Оригинал не изменится."),
    ("confirm_dup_bin", "Poslaću %1 suvišnih kopija u Korpu. Odatle mogu da se vrate.", "I'll send %1 extra copies to the Recycle Bin. They can be restored from there.", "Ich lege %1 überzählige Kopien in den Papierkorb. Von dort sind sie wiederherstellbar.", "Отправлю %1 лишних копий в Корзину. Оттуда их можно восстановить."),
    ("confirm_dup_folder", "Premestiću %1 suvišnih kopija u folder „_MarkSync duplikati”. Kad proveriš, taj folder možeš sam da obrišeš.", "I'll move %1 extra copies into the “_MarkSync duplikati” folder. Once you've checked, you can delete that folder yourself.", "Ich verschiebe %1 überzählige Kopien in den Ordner „_MarkSync duplikati”. Nach Prüfung kannst du ihn selbst löschen.", "Перемещу %1 лишних копий в папку «_MarkSync duplikati». После проверки можете удалить её сами."),
    ("confirm_undo", "Vratiću sve fajlove iz poslednjeg sređivanja na mesta gde su bili.", "I'll move all files from the last organize back to where they were.", "Ich verschiebe alle Dateien des letzten Ordnens zurück an ihren alten Platz.", "Верну все файлы последнего упорядочивания на прежние места."),
    ("rep_copy", "Kopiranje završeno", "Copying finished", "Kopieren abgeschlossen", "Копирование завершено"),
    ("rep_org", "Sređivanje završeno", "Organizing finished", "Ordnen abgeschlossen", "Упорядочивание завершено"),
    ("rep_undo", "Poništavanje završeno", "Undo finished", "Rückgängig abgeschlossen", "Отмена завершена"),
    ("rep_dup", "Višak uklonjen", "Extras removed", "Überzähliges entfernt", "Лишнее убрано"),
    ("err_title", "Greška", "Error", "Fehler", "Ошибка"),
    ("all_done", "Gotovo!", "Done!", "Fertig!", "Готово!"),
    ("was_cancelled", "Prekinuto pre kraja.", "Stopped before the end.", "Vor dem Ende abgebrochen.", "Прервано до завершения."),
    ("rep_ok", "Uspešno: %1 fajlova (%2)", "Done: %1 files (%2)", "Erledigt: %1 Dateien (%2)", "Успешно: %1 файлов (%2)"),
    ("rep_errors", "Nije uspelo: %1", "Failed: %1", "Fehlgeschlagen: %1", "Не удалось: %1"),
    ("open_folder", "Otvori folder", "Open folder", "Ordner öffnen", "Открыть папку"),
    ("err_no_dest", "Izaberi folder u koji se kopira.", "Choose the destination folder.", "Wähle den Zielordner.", "Выберите папку назначения."),
    ("err_journal", "Zapis za poništavanje nije pronađen.", "The undo record was not found.", "Rückgängig-Protokoll nicht gefunden.", "Запись для отмены не найдена."),
    ("err_exists", "Na starom mestu sada postoji drugi fajl — preskočeno.", "Another file now exists at the old place — skipped.", "Am alten Ort liegt jetzt eine andere Datei – übersprungen.", "На старом месте теперь другой файл — пропущено."),
    ("err_trash", "Nije moglo u Korpu (disk možda nema Korpu) — izaberi „Premesti u folder”.", "Couldn't move to the Recycle Bin (the disk may not have one) — choose “Move to folder”.", "Papierkorb nicht möglich (Laufwerk hat evtl. keinen) – wähle „In Ordner verschieben”.", "Не удалось отправить в Корзину (на диске её может не быть) — выберите «Переместить в папку»."),
];
