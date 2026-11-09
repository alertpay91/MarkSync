//! Background thumbnail loading (EXIF preview first, full decode as fallback).

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

const SIZE: u32 = 128;
const MAX_CACHED: usize = 600;
const MAX_FULL_DECODE: u64 = 60 * 1024 * 1024;

type Queue = Arc<(Mutex<Vec<PathBuf>>, Condvar)>;

pub struct Thumbs {
    queue: Queue,
    rx: Receiver<(PathBuf, Option<ColorImage>)>,
    cache: HashMap<PathBuf, Option<TextureHandle>>,
    order: VecDeque<PathBuf>,
    pending: HashSet<PathBuf>,
}

impl Thumbs {
    pub fn new(ctx: &egui::Context) -> Self {
        let queue: Queue = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let (tx, rx) = channel();
        for _ in 0..3 {
            let q = queue.clone();
            let tx: Sender<_> = tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || loop {
                let path = {
                    let (lock, cv) = &*q;
                    let mut v = lock.lock().unwrap();
                    while v.is_empty() {
                        v = cv.wait(v).unwrap();
                    }
                    v.pop().unwrap() // newest request first (what is on screen now)
                };
                let img = load(&path);
                if tx.send((path, img)).is_err() {
                    break;
                }
                ctx.request_repaint();
            });
        }
        Self { queue, rx, cache: HashMap::new(), order: VecDeque::new(), pending: HashSet::new() }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((path, img)) = self.rx.try_recv() {
            self.pending.remove(&path);
            let tex = img.map(|i| ctx.load_texture(path.to_string_lossy(), i, TextureOptions::LINEAR));
            self.cache.insert(path.clone(), tex);
            self.order.push_back(path);
            while self.order.len() > MAX_CACHED {
                if let Some(old) = self.order.pop_front() {
                    self.cache.remove(&old);
                }
            }
        }
    }

    pub fn get(&mut self, path: &Path) -> Option<&TextureHandle> {
        if let Some(t) = self.cache.get(path) {
            return t.as_ref();
        }
        if self.pending.insert(path.to_path_buf()) {
            let (lock, cv) = &*self.queue;
            let mut v = lock.lock().unwrap();
            v.push(path.to_path_buf());
            // drop requests for rows that scrolled away long ago
            if v.len() > 80 {
                let cut = v.len() - 80;
                let old: Vec<PathBuf> = v.drain(..cut).collect();
                for o in old {
                    self.pending.remove(&o);
                }
            }
            cv.notify_one();
        }
        None
    }
}

fn load(path: &Path) -> Option<ColorImage> {
    let (thumb, orientation) = exif_thumb(path);
    let img = match thumb {
        Some(i) => i,
        None => {
            if std::fs::metadata(path).ok()?.len() > MAX_FULL_DECODE {
                return None;
            }
            image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?
        }
    };
    let img = img.thumbnail(SIZE, SIZE);
    let img = match orientation {
        3 => img.rotate180(),
        6 => img.rotate90(),
        8 => img.rotate270(),
        _ => img,
    };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

fn exif_thumb(path: &Path) -> (Option<image::DynamicImage>, u32) {
    use exif::{In, Tag};
    let Ok(f) = std::fs::File::open(path) else { return (None, 1) };
    let Ok(ex) = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(f)) else {
        return (None, 1);
    };
    let orientation = ex
        .get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .unwrap_or(1);
    let off = ex.get_field(Tag::JPEGInterchangeFormat, In::THUMBNAIL).and_then(|f| f.value.get_uint(0));
    let len = ex.get_field(Tag::JPEGInterchangeFormatLength, In::THUMBNAIL).and_then(|f| f.value.get_uint(0));
    if let (Some(off), Some(len)) = (off, len) {
        let buf = ex.buf();
        let (o, l) = (off as usize, len as usize);
        if o + l <= buf.len() && l > 100 {
            if let Ok(img) = image::load_from_memory(&buf[o..o + l]) {
                // tiny previews look bad when scaled up; use them only if reasonably large
                if img.width() >= 120 || img.height() >= 120 {
                    return (Some(img), orientation);
                }
            }
        }
    }
    (None, orientation)
}
