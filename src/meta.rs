//! Photo/video metadata: capture date, GPS position, camera; offline place names.

use crate::fsutil::Kind;
use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, TimeZone};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::OnceLock;
use std::time::SystemTime;

/// Where the date came from — the last one is only a guess.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DateFrom {
    #[default]
    Unknown,
    Exif,
    Video,
    Name,
    FileTime,
}

#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub date: Option<NaiveDateTime>,
    pub date_from: DateFrom,
    pub gps: Option<(f64, f64)>,
    pub camera: Option<String>,
}

fn has_exif(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("jpg" | "jpeg" | "jpe" | "tif" | "tiff" | "heic" | "heif" | "png" | "webp" | "dng"
            | "avif")
    )
}

fn is_mp4_like(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("mp4" | "mov" | "m4v" | "3gp" | "3g2" | "m4a")
    )
}

pub fn read_meta(path: &Path, kind: Kind, mtime: SystemTime) -> Meta {
    let mut m = Meta::default();
    if kind == Kind::Image && has_exif(path) {
        read_exif(path, &mut m);
        if m.date.is_some() {
            m.date_from = DateFrom::Exif;
        }
    }
    if m.date.is_none() && is_mp4_like(path) {
        m.date = mp4_creation(path);
        if m.date.is_some() {
            m.date_from = DateFrom::Video;
        }
    }
    if m.date.is_none() {
        m.date = date_from_name(path.file_name().and_then(|n| n.to_str()).unwrap_or(""));
        if m.date.is_some() {
            m.date_from = DateFrom::Name;
        }
    }
    if m.date.is_none() {
        let dt: DateTime<Local> = mtime.into();
        m.date = Some(dt.naive_local());
        m.date_from = DateFrom::FileTime;
    }
    m
}

/// Metadata from the start of a file (phones: we don't read the whole file just for this).
pub fn read_meta_bytes(name: &str, head: &[u8], kind: Kind, mtime: SystemTime) -> Meta {
    let mut m = Meta::default();
    if kind == Kind::Image && has_exif(Path::new(name)) {
        if let Ok(ex) = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(head)) {
            exif_fields(&ex, &mut m);
            if m.date.is_some() {
                m.date_from = DateFrom::Exif;
            }
        }
    }
    if m.date.is_none() {
        m.date = date_from_name(name);
        if m.date.is_some() {
            m.date_from = DateFrom::Name;
        }
    }
    if m.date.is_none() && mtime > std::time::UNIX_EPOCH {
        let dt: DateTime<Local> = mtime.into();
        m.date = Some(dt.naive_local());
        m.date_from = DateFrom::FileTime;
    }
    m
}

fn read_exif(path: &Path, m: &mut Meta) {
    let Ok(f) = File::open(path) else { return };
    let Ok(ex) = exif::Reader::new().read_from_container(&mut BufReader::new(f)) else { return };
    exif_fields(&ex, m);
}

fn exif_fields(ex: &exif::Exif, m: &mut Meta) {
    use exif::{In, Tag, Value};

    for tag in [Tag::DateTimeOriginal, Tag::DateTimeDigitized, Tag::DateTime] {
        if let Some(field) = ex.get_field(tag, In::PRIMARY) {
            if let Value::Ascii(ref v) = field.value {
                if let Some(bytes) = v.first() {
                    if let Ok(d) = exif::DateTime::from_ascii(bytes) {
                        if let Some(nd) = NaiveDate::from_ymd_opt(d.year as i32, d.month as u32, d.day as u32) {
                            if d.year > 1980 {
                                m.date = nd.and_hms_opt(d.hour as u32, d.minute as u32, d.second as u32);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    let coord = |t: Tag, r: Tag| -> Option<f64> {
        let f = ex.get_field(t, In::PRIMARY)?;
        let Value::Rational(ref v) = f.value else { return None };
        if v.len() < 3 {
            return None;
        }
        let mut deg = v[0].to_f64() + v[1].to_f64() / 60.0 + v[2].to_f64() / 3600.0;
        if !deg.is_finite() {
            return None;
        }
        if let Some(rf) = ex.get_field(r, In::PRIMARY) {
            let s = rf.display_value().to_string();
            if s.contains('S') || s.contains('W') {
                deg = -deg;
            }
        }
        Some(deg)
    };
    if let (Some(lat), Some(lon)) = (
        coord(Tag::GPSLatitude, Tag::GPSLatitudeRef),
        coord(Tag::GPSLongitude, Tag::GPSLongitudeRef),
    ) {
        if (lat.abs() > 0.0001 || lon.abs() > 0.0001) && lat.abs() <= 90.0 && lon.abs() <= 180.0 {
            m.gps = Some((lat, lon));
        }
    }

    let text = |t: Tag| -> Option<String> {
        let f = ex.get_field(t, In::PRIMARY)?;
        let s = f.display_value().to_string().trim_matches(|c: char| c == '"' || c.is_whitespace()).to_string();
        (!s.is_empty()).then_some(s)
    };
    let make = text(Tag::Make);
    let model = text(Tag::Model);
    m.camera = match (make, model) {
        (Some(mk), Some(md)) => {
            if md.to_lowercase().starts_with(&mk.to_lowercase()) {
                Some(md)
            } else {
                let mk = mk.split_whitespace().next().unwrap_or(&mk).to_string();
                Some(format!("{} {}", capitalize(&mk), md))
            }
        }
        (None, Some(md)) => Some(md),
        (Some(mk), None) => Some(capitalize(&mk)),
        _ => None,
    };
}

fn capitalize(s: &str) -> String {
    let lower = s.to_lowercase();
    if s.chars().all(|c| !c.is_lowercase()) || s == lower {
        let mut c = lower.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => String::new(),
        }
    } else {
        s.to_string()
    }
}

/// Creation time from the `moov/mvhd` atom of MP4/MOV files.
fn mp4_creation(path: &Path) -> Option<NaiveDateTime> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut pos = 0u64;
    // find top-level moov
    let (moov_start, moov_end) = loop {
        if pos + 8 > len {
            return None;
        }
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 8];
        f.read_exact(&mut h).ok()?;
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let mut hdr = 8;
        if size == 1 {
            let mut b = [0u8; 8];
            f.read_exact(&mut b).ok()?;
            size = u64::from_be_bytes(b);
            hdr = 16;
        } else if size == 0 {
            size = len - pos;
        }
        if size < hdr {
            return None;
        }
        if &h[4..8] == b"moov" {
            break (pos + hdr, pos + size);
        }
        pos += size;
    };
    let mut p = moov_start;
    while p + 8 <= moov_end {
        f.seek(SeekFrom::Start(p)).ok()?;
        let mut h = [0u8; 8];
        f.read_exact(&mut h).ok()?;
        let size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        if &h[4..8] == b"mvhd" {
            let mut vf = [0u8; 4];
            f.read_exact(&mut vf).ok()?;
            let secs = if vf[0] == 1 {
                let mut b = [0u8; 8];
                f.read_exact(&mut b).ok()?;
                u64::from_be_bytes(b)
            } else {
                let mut b = [0u8; 4];
                f.read_exact(&mut b).ok()?;
                u32::from_be_bytes(b) as u64
            };
            // seconds since 1904-01-01 (UTC)
            const OFFSET: i64 = 2_082_844_800;
            let unix = secs as i64 - OFFSET;
            if unix < 315_532_800 {
                return None; // unset (1904) or before 1980
            }
            let utc = chrono::Utc.timestamp_opt(unix, 0).single()?;
            return Some(utc.with_timezone(&Local).naive_local());
        }
        if size < 8 {
            return None;
        }
        p += size;
    }
    None
}

/// Dates in names from phones and messengers: IMG_20240715_..., VID-20240715-WA0001, 2024-07-15 ...
pub fn date_from_name(name: &str) -> Option<NaiveDateTime> {
    let d = date_from_name_raw(name)?;
    // Google Pixel names (PXL_20250905_094413504) are in UTC
    if name.starts_with("PXL_") {
        return Some(d.and_utc().with_timezone(&Local).naive_local());
    }
    Some(d)
}

fn date_from_name_raw(name: &str) -> Option<NaiveDateTime> {
    let b: Vec<char> = name.chars().collect();
    let num = |s: &[char]| -> Option<u32> {
        if s.iter().all(|c| c.is_ascii_digit()) {
            s.iter().collect::<String>().parse().ok()
        } else {
            None
        }
    };
    let valid = |y: u32, m: u32, d: u32| -> Option<NaiveDate> {
        if !(1990..=2100).contains(&y) {
            return None;
        }
        NaiveDate::from_ymd_opt(y as i32, m, d)
    };
    let mut i = 0;
    while i + 8 <= b.len() {
        let boundary_ok = i == 0 || !b[i - 1].is_ascii_digit();
        if boundary_ok {
            // YYYYMMDD
            if let (Some(y), Some(m), Some(d)) = (num(&b[i..i + 4]), num(&b[i + 4..i + 6]), num(&b[i + 6..i + 8])) {
                if let Some(nd) = valid(y, m, d) {
                    // optional _HHMMSS
                    let mut t = nd.and_hms_opt(12, 0, 0);
                    if i + 15 <= b.len() && !b[i + 8].is_ascii_digit() {
                        if let (Some(h), Some(mi), Some(s)) =
                            (num(&b[i + 9..i + 11]), num(&b[i + 11..i + 13]), num(&b[i + 13..i + 15]))
                        {
                            if let Some(tt) = nd.and_hms_opt(h, mi, s) {
                                t = Some(tt);
                            }
                        }
                    }
                    return t;
                }
            }
            // YYYY-MM-DD / YYYY_MM_DD / YYYY.MM.DD
            if i + 10 <= b.len() && matches!(b[i + 4], '-' | '_' | '.') && b[i + 7] == b[i + 4] {
                if let (Some(y), Some(m), Some(d)) = (num(&b[i..i + 4]), num(&b[i + 5..i + 7]), num(&b[i + 8..i + 10])) {
                    if let Some(nd) = valid(y, m, d) {
                        return nd.and_hms_opt(12, 0, 0);
                    }
                }
            }
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Places
// ---------------------------------------------------------------------------

fn geocoder() -> &'static reverse_geocoder::ReverseGeocoder {
    static G: OnceLock<reverse_geocoder::ReverseGeocoder> = OnceLock::new();
    G.get_or_init(reverse_geocoder::ReverseGeocoder::new)
}

/// Loads the place database in the background so the first lookup is fast.
pub fn warm_up_geocoder() {
    std::thread::spawn(|| {
        let _ = geocoder();
    });
}

pub struct Place {
    pub country: String,
    pub city: String,
}

pub fn place(lat: f64, lon: f64) -> Place {
    let r = geocoder().search((lat, lon));
    let rec = r.record;
    Place { country: country_name(&rec.cc), city: rec.name.clone() }
}

pub fn year_month(d: &NaiveDateTime) -> (i32, u32, u32) {
    (d.year(), d.month(), d.day())
}

/// Country name for an ISO code, in the current language (falls back to English, then the code).
pub fn country_name(cc: &str) -> String {
    let lang = crate::i18n::lang();
    for (code, sr, en, de, ru) in COUNTRIES {
        if *code == cc {
            return match lang {
                crate::i18n::Lang::SrLatn => sr.to_string(),
                crate::i18n::Lang::SrCyrl => crate::i18n::to_cyrillic(sr),
                crate::i18n::Lang::En => en.to_string(),
                crate::i18n::Lang::De => de.to_string(),
                crate::i18n::Lang::Ru => ru.to_string(),
            };
        }
    }
    cc.to_string()
}

#[rustfmt::skip]
const COUNTRIES: &[(&str, &str, &str, &str, &str)] = &[
    ("RS", "Srbija", "Serbia", "Serbien", "Сербия"),
    ("ME", "Crna Gora", "Montenegro", "Montenegro", "Черногория"),
    ("BA", "Bosna i Hercegovina", "Bosnia and Herzegovina", "Bosnien und Herzegowina", "Босния и Герцеговина"),
    ("HR", "Hrvatska", "Croatia", "Kroatien", "Хорватия"),
    ("SI", "Slovenija", "Slovenia", "Slowenien", "Словения"),
    ("MK", "Severna Makedonija", "North Macedonia", "Nordmazedonien", "Северная Македония"),
    ("XK", "Kosovo", "Kosovo", "Kosovo", "Косово"),
    ("AL", "Albanija", "Albania", "Albanien", "Албания"),
    ("BG", "Bugarska", "Bulgaria", "Bulgarien", "Болгария"),
    ("RO", "Rumunija", "Romania", "Rumänien", "Румыния"),
    ("HU", "Mađarska", "Hungary", "Ungarn", "Венгрия"),
    ("GR", "Grčka", "Greece", "Griechenland", "Греция"),
    ("TR", "Turska", "Turkey", "Türkei", "Турция"),
    ("CY", "Kipar", "Cyprus", "Zypern", "Кипр"),
    ("AT", "Austrija", "Austria", "Österreich", "Австрия"),
    ("DE", "Nemačka", "Germany", "Deutschland", "Германия"),
    ("CH", "Švajcarska", "Switzerland", "Schweiz", "Швейцария"),
    ("IT", "Italija", "Italy", "Italien", "Италия"),
    ("FR", "Francuska", "France", "Frankreich", "Франция"),
    ("ES", "Španija", "Spain", "Spanien", "Испания"),
    ("PT", "Portugal", "Portugal", "Portugal", "Португалия"),
    ("GB", "Velika Britanija", "United Kingdom", "Vereinigtes Königreich", "Великобритания"),
    ("IE", "Irska", "Ireland", "Irland", "Ирландия"),
    ("NL", "Holandija", "Netherlands", "Niederlande", "Нидерланды"),
    ("BE", "Belgija", "Belgium", "Belgien", "Бельгия"),
    ("LU", "Luksemburg", "Luxembourg", "Luxemburg", "Люксембург"),
    ("DK", "Danska", "Denmark", "Dänemark", "Дания"),
    ("SE", "Švedska", "Sweden", "Schweden", "Швеция"),
    ("NO", "Norveška", "Norway", "Norwegen", "Норвегия"),
    ("FI", "Finska", "Finland", "Finnland", "Финляндия"),
    ("IS", "Island", "Iceland", "Island", "Исландия"),
    ("PL", "Poljska", "Poland", "Polen", "Польша"),
    ("CZ", "Češka", "Czechia", "Tschechien", "Чехия"),
    ("SK", "Slovačka", "Slovakia", "Slowakei", "Словакия"),
    ("UA", "Ukrajina", "Ukraine", "Ukraine", "Украина"),
    ("BY", "Belorusija", "Belarus", "Belarus", "Беларусь"),
    ("RU", "Rusija", "Russia", "Russland", "Россия"),
    ("MD", "Moldavija", "Moldova", "Moldau", "Молдавия"),
    ("LT", "Litvanija", "Lithuania", "Litauen", "Литва"),
    ("LV", "Letonija", "Latvia", "Lettland", "Латвия"),
    ("EE", "Estonija", "Estonia", "Estland", "Эстония"),
    ("MT", "Malta", "Malta", "Malta", "Мальта"),
    ("MC", "Monako", "Monaco", "Monaco", "Монако"),
    ("SM", "San Marino", "San Marino", "San Marino", "Сан-Марино"),
    ("VA", "Vatikan", "Vatican City", "Vatikanstadt", "Ватикан"),
    ("AD", "Andora", "Andorra", "Andorra", "Андорра"),
    ("LI", "Lihtenštajn", "Liechtenstein", "Liechtenstein", "Лихтенштейн"),
    ("US", "SAD", "United States", "USA", "США"),
    ("CA", "Kanada", "Canada", "Kanada", "Канада"),
    ("MX", "Meksiko", "Mexico", "Mexiko", "Мексика"),
    ("BR", "Brazil", "Brazil", "Brasilien", "Бразилия"),
    ("AR", "Argentina", "Argentina", "Argentinien", "Аргентина"),
    ("CU", "Kuba", "Cuba", "Kuba", "Куба"),
    ("DO", "Dominikanska Republika", "Dominican Republic", "Dominikanische Republik", "Доминиканская Республика"),
    ("AU", "Australija", "Australia", "Australien", "Австралия"),
    ("NZ", "Novi Zeland", "New Zealand", "Neuseeland", "Новая Зеландия"),
    ("CN", "Kina", "China", "China", "Китай"),
    ("JP", "Japan", "Japan", "Japan", "Япония"),
    ("KR", "Južna Koreja", "South Korea", "Südkorea", "Южная Корея"),
    ("TH", "Tajland", "Thailand", "Thailand", "Таиланд"),
    ("VN", "Vijetnam", "Vietnam", "Vietnam", "Вьетнам"),
    ("ID", "Indonezija", "Indonesia", "Indonesien", "Индонезия"),
    ("MY", "Malezija", "Malaysia", "Malaysia", "Малайзия"),
    ("SG", "Singapur", "Singapore", "Singapur", "Сингапур"),
    ("IN", "Indija", "India", "Indien", "Индия"),
    ("LK", "Šri Lanka", "Sri Lanka", "Sri Lanka", "Шри-Ланка"),
    ("MV", "Maldivi", "Maldives", "Malediven", "Мальдивы"),
    ("AE", "UAE", "United Arab Emirates", "Vereinigte Arabische Emirate", "ОАЭ"),
    ("QA", "Katar", "Qatar", "Katar", "Катар"),
    ("SA", "Saudijska Arabija", "Saudi Arabia", "Saudi-Arabien", "Саудовская Аравия"),
    ("IL", "Izrael", "Israel", "Israel", "Израиль"),
    ("JO", "Jordan", "Jordan", "Jordanien", "Иордания"),
    ("EG", "Egipat", "Egypt", "Ägypten", "Египет"),
    ("TN", "Tunis", "Tunisia", "Tunesien", "Тунис"),
    ("MA", "Maroko", "Morocco", "Marokko", "Марокко"),
    ("ZA", "Južna Afrika", "South Africa", "Südafrika", "ЮАР"),
    ("KE", "Kenija", "Kenya", "Kenia", "Кения"),
    ("TZ", "Tanzanija", "Tanzania", "Tansania", "Танзания"),
    ("GE", "Gruzija", "Georgia", "Georgien", "Грузия"),
    ("AM", "Jermenija", "Armenia", "Armenien", "Армения"),
    ("AZ", "Azerbejdžan", "Azerbaijan", "Aserbaidschan", "Азербайджан"),
    ("KZ", "Kazahstan", "Kazakhstan", "Kasachstan", "Казахстан"),
];
