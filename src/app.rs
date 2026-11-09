//! The user interface.

use crate::dupes::{self, DupGroup};
use crate::fsutil::{self, Drive, Kind, KindFilter, Known};
use crate::i18n::{self, fmt_date, fmt_num, fmt_size, t, tf, Lang};
use crate::job::{Progress, Report};
use crate::organize::{self, OrgMode, OrgPlan, Scheme};
use crate::phone;
use crate::settings::{DupAction, LayoutChoice, Settings, SourceCfg, Theme};
use crate::sync::{self, Action, Item, Status};
use crate::thumbs::Thumbs;
use crate::tree::FolderTree;
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, FontFamily, FontId, Frame, Layout, Margin, Rect, RichText, Rounding,
    Sense, Stroke, Ui,
};
use egui_extras::{Column, TableBuilder};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const AUTHOR: &str = "Marko Stevanović";
const DONATE_EMAIL: &str = "alertpay91@gmail.com";

/// PayPal's "Donate" button doesn't work for Serbia; a plain payment ("Buy Now") link does.
fn coffee_url(eur: u32) -> String {
    format!("https://www.paypal.com/cgi-bin/webscr?cmd=_xclick&business=alertpay91%40gmail.com&item_name=Coffee%20for%20MarkSync&amount={eur}.00&currency_code=EUR&no_shipping=1")
}

pub const ACCENT: Color32 = Color32::from_rgb(92, 124, 250);
const GREEN: Color32 = Color32::from_rgb(46, 196, 120);
const AMBER: Color32 = Color32::from_rgb(240, 170, 40);
const RED: Color32 = Color32::from_rgb(236, 88, 80);
const GRAY: Color32 = Color32::from_rgb(130, 140, 155);
const PURPLE: Color32 = Color32::from_rgb(160, 110, 230);

fn status_color(s: Status) -> Color32 {
    match s {
        Status::New => GREEN,
        Status::Newer => AMBER,
        Status::Conflict => RED,
        Status::Exists => GRAY,
        Status::DupInSource => PURPLE,
    }
}

fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Sync,
    Organize,
    Dupes,
    Settings,
}

enum Done {
    Compare(Result<Vec<Item>, String>),
    Copy(Report),
    Plan(Result<OrgPlan, String>),
    Organized(Report, Option<PathBuf>),
    Undo(Report),
    Dupes(Result<Vec<DupGroup>, String>),
    DupRemoved(Report),
}

struct Running {
    p: Arc<Progress>,
    result: Arc<Mutex<Option<Done>>>,
    title: &'static str,
    started: Instant,
}

enum PMsg {
    Devices(Vec<phone::Device>),
    Folders(u64, Result<Vec<phone::Entry>, String>),
    Camera(Option<Vec<String>>),
}

/// The "pick a folder on the phone" window.
struct PhoneBrowser {
    devices: Option<Vec<phone::Device>>,
    dev: Option<phone::Device>,
    /// (object id, name) from the phone root down to the folder shown
    stack: Vec<(String, String)>,
    folders: Option<Result<Vec<phone::Entry>, String>>,
    camera: Option<Option<Vec<String>>>,
    gen: u64,
    tx: std::sync::mpsc::Sender<PMsg>,
    rx: std::sync::mpsc::Receiver<PMsg>,
}

impl PhoneBrowser {
    fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut b = Self { devices: None, dev: None, stack: Vec::new(), folders: None, camera: None, gen: 0, tx, rx };
        b.load_devices(ctx);
        b
    }
    fn load_devices(&mut self, ctx: &egui::Context) {
        self.devices = None;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            phone::reset();
            let _ = tx.send(PMsg::Devices(phone::list_devices()));
            ctx.request_repaint();
        });
    }
    fn open_device(&mut self, ctx: &egui::Context, d: phone::Device) {
        self.dev = Some(d.clone());
        self.stack.clear();
        self.camera = None;
        self.load_folders(ctx);
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut found = None;
            if let Ok(st) = phone::children(&d.id, phone::WPD_DEVICE_OBJECT_ID_STR) {
                'outer: for s in st.into_iter().filter(|e| e.is_folder) {
                    // Android: DCIM\Camera; iPhone: DCIM\100APPLE, 101APPLE… → the whole DCIM
                    for path in [
                        vec![s.name.clone(), "DCIM".to_string(), "Camera".to_string()],
                        vec![s.name.clone(), "DCIM".to_string()],
                    ] {
                        if phone::resolve(&d.id, &path).is_ok() {
                            found = Some(path);
                            break 'outer;
                        }
                    }
                }
            }
            let _ = tx.send(PMsg::Camera(found));
            ctx.request_repaint();
        });
    }
    fn load_folders(&mut self, ctx: &egui::Context) {
        let Some(d) = self.dev.clone() else { return };
        self.gen += 1;
        self.folders = None;
        let gen = self.gen;
        let parent = self.stack.last().map(|s| s.0.clone()).unwrap_or_else(|| phone::WPD_DEVICE_OBJECT_ID_STR.into());
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = phone::children(&d.id, &parent)
                .map(|v| v.into_iter().filter(|e| e.is_folder && !e.name.starts_with('.')).collect())
                .map_err(|e| e.to_string());
            let _ = tx.send(PMsg::Folders(gen, r));
            ctx.request_repaint();
        });
    }
    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                PMsg::Devices(d) => self.devices = Some(d),
                PMsg::Folders(g, r) if g == self.gen => self.folders = Some(r),
                PMsg::Folders(..) => {}
                PMsg::Camera(c) => self.camera = Some(c),
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    Copy,
    Organize,
    RemoveDups,
    Undo,
}

pub struct MarkSync {
    s: Settings,
    tab: Tab,
    trees: Vec<FolderTree>,
    drives: Vec<Drive>,
    drives_time: Instant,
    thumbs: Thumbs,
    job: Option<Running>,
    report: Option<(&'static str, Report)>,
    confirm: Option<Confirm>,
    toast: Option<(String, Instant)>,
    applied_theme: Option<Theme>,
    phone_ui: Option<PhoneBrowser>,
    // sync
    items: Vec<Item>,
    compared: bool,
    hidden_status: HashSet<Status>,
    search: String,
    view: Vec<usize>,
    view_dirty: bool,
    warnings: Vec<String>,
    // organize
    org_plan: Option<OrgPlan>,
    // duplicates
    groups: Vec<DupGroup>,
    dup_rows: Vec<(usize, Option<usize>)>,
    dup_searched: bool,
}

impl MarkSync {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::setup_fonts(&cc.egui_ctx);
        let s = Settings::load();
        i18n::set_lang(Lang::from_code(s.lang.as_deref().unwrap_or("srlatn")));
        let trees = s
            .sources
            .iter()
            .filter(|c| c.root.is_dir())
            .map(|c| FolderTree::new(c.root.clone(), c.states.clone()))
            .collect();
        let mut hidden_status = HashSet::new();
        hidden_status.insert(Status::Exists);
        hidden_status.insert(Status::DupInSource);
        Self {
            s,
            tab: Tab::Sync,
            trees,
            drives: fsutil::list_drives(),
            drives_time: Instant::now(),
            thumbs: Thumbs::new(&cc.egui_ctx),
            job: None,
            report: None,
            confirm: None,
            toast: None,
            applied_theme: None,
            phone_ui: None,
            items: Vec::new(),
            compared: false,
            hidden_status,
            search: String::new(),
            view: Vec::new(),
            view_dirty: true,
            warnings: Vec::new(),
            org_plan: None,
            groups: Vec::new(),
            dup_rows: Vec::new(),
            dup_searched: false,
        }
    }

    fn save(&mut self) {
        self.s.sources =
            self.trees.iter().map(|t| SourceCfg { root: t.root.clone(), states: t.states_vec() }).collect();
        self.s.save();
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn spawn(&mut self, ctx: &egui::Context, title: &'static str, f: impl FnOnce(&Progress) -> Done + Send + 'static) {
        let p = Arc::new(Progress::default());
        let result = Arc::new(Mutex::new(None));
        let (p2, r2, c2) = (p.clone(), result.clone(), ctx.clone());
        std::thread::spawn(move || {
            let d = f(&p2);
            *r2.lock().unwrap() = Some(d);
            c2.request_repaint();
        });
        self.job = Some(Running { p, result, title, started: Instant::now() });
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let done = job.result.lock().unwrap().take();
        let Some(done) = done else {
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        };
        let warnings = std::mem::take(&mut *job.p.warnings.lock().unwrap());
        self.job = None;
        match done {
            Done::Compare(Ok(items)) => {
                self.warnings = warnings;
                self.items = items;
                self.compared = true;
                self.view_dirty = true;
            }
            Done::Compare(Err(e)) | Done::Plan(Err(e)) | Done::Dupes(Err(e)) => {
                if e != t("cancelled") {
                    self.report =
                        Some(("err_title", Report { errors: vec![(String::new(), e)], ..Default::default() }));
                }
            }
            Done::Copy(rep) => {
                for (&i, to) in rep.done_idx.iter().zip(&rep.done_dest) {
                    if let Some(it) = self.items.get_mut(i) {
                        it.status = Status::Exists;
                        it.selected = false;
                        it.same_as = Some(to.clone());
                    }
                }
                self.view_dirty = true;
                self.drives = fsutil::list_drives();
                self.report = Some(("rep_copy", rep));
            }
            Done::Plan(Ok(plan)) => self.org_plan = Some(plan),
            Done::Organized(rep, journal) => {
                if journal.is_some() {
                    self.s.last_undo = journal;
                    self.save();
                }
                self.report = Some(("rep_org", rep));
            }
            Done::Undo(rep) => {
                self.s.last_undo = None;
                self.save();
                self.report = Some(("rep_undo", rep));
            }
            Done::Dupes(Ok(groups)) => {
                self.groups = groups;
                self.dup_searched = true;
                self.rebuild_dup_rows();
            }
            Done::DupRemoved(rep) => {
                self.groups.clear();
                self.dup_rows.clear();
                self.dup_searched = false;
                self.report = Some(("rep_dup", rep));
            }
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.applied_theme == Some(self.s.theme) {
            return;
        }
        self.applied_theme = Some(self.s.theme);
        let dark = self.s.theme == Theme::Dark;
        let rgb = Color32::from_rgb;
        let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        if dark {
            v.panel_fill = rgb(16, 18, 24);
            v.window_fill = rgb(26, 29, 37);
            v.extreme_bg_color = rgb(11, 12, 16);
            v.faint_bg_color = rgb(22, 25, 32);
            v.widgets.noninteractive.bg_fill = rgb(26, 29, 37);
            v.widgets.noninteractive.weak_bg_fill = rgb(26, 29, 37);
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, rgb(40, 44, 56));
            v.widgets.inactive.bg_fill = rgb(38, 42, 54);
            v.widgets.inactive.weak_bg_fill = rgb(38, 42, 54);
            v.widgets.hovered.bg_fill = rgb(50, 56, 72);
            v.widgets.hovered.weak_bg_fill = rgb(50, 56, 72);
            v.widgets.active.weak_bg_fill = rgb(60, 68, 90);
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, rgb(200, 205, 215));
            v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, rgb(225, 228, 235));
        } else {
            v.panel_fill = rgb(242, 244, 248);
            v.window_fill = rgb(255, 255, 255);
            v.extreme_bg_color = rgb(232, 235, 241);
            v.faint_bg_color = rgb(246, 247, 250);
            v.widgets.noninteractive.bg_fill = rgb(255, 255, 255);
            v.widgets.noninteractive.weak_bg_fill = rgb(255, 255, 255);
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, rgb(222, 226, 234));
            v.widgets.inactive.bg_fill = rgb(232, 235, 242);
            v.widgets.inactive.weak_bg_fill = rgb(232, 235, 242);
            v.widgets.hovered.bg_fill = rgb(218, 223, 235);
            v.widgets.hovered.weak_bg_fill = rgb(218, 223, 235);
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, rgb(50, 55, 65));
            v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, rgb(30, 34, 42));
        }
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.5_f32, Color32::WHITE);
        v.hyperlink_color = ACCENT;
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.rounding = Rounding::same(8.0);
        }
        v.widgets.inactive.bg_stroke = Stroke::NONE;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.6));
        v.window_rounding = Rounding::same(16.0);
        v.menu_rounding = Rounding::same(10.0);
        v.window_shadow.blur = 30.0;
        v.striped = true;

        let mut style = (*ctx.style()).clone();
        style.visuals = v;
        style.interaction.selectable_labels = false;
        style.spacing.item_spacing = vec2(10.0, 8.0);
        style.spacing.button_padding = vec2(14.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.icon_width = 20.0;
        style.spacing.icon_width_inner = 12.0;
        use egui::TextStyle::*;
        style.text_styles.insert(Body, FontId::proportional(15.0));
        style.text_styles.insert(Button, FontId::proportional(15.0));
        style.text_styles.insert(Heading, FontId::new(26.0, semibold()));
        style.text_styles.insert(Small, FontId::proportional(12.5));
        style.text_styles.insert(Monospace, FontId::monospace(13.0));
        ctx.set_style(style);
    }
}

// ---------------------------------------------------------------------------
// small widgets
// ---------------------------------------------------------------------------

fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::none()
        .fill(ui.visuals().window_fill)
        .rounding(14.0)
        .inner_margin(Margin::same(18.0))
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .show(ui, add)
        .inner
}

fn big_button(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    let b = egui::Button::new(RichText::new(text).size(17.0).family(semibold()).color(Color32::WHITE))
        .fill(if enabled { ACCENT } else { ACCENT.gamma_multiply(0.35) })
        .rounding(12.0)
        .min_size(vec2(240.0, 46.0));
    ui.add_enabled(enabled, b)
}

fn chip(ui: &mut Ui, on: bool, text: &str) -> egui::Response {
    let rt = if on { RichText::new(text).color(Color32::WHITE) } else { RichText::new(text) };
    let b = egui::Button::new(rt).rounding(20.0).fill(if on { ACCENT } else { ui.visuals().widgets.inactive.weak_bg_fill });
    ui.add(b)
}

fn section_title(ui: &mut Ui, num: &str, title: &str, sub: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 14.0, ACCENT);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, num, FontId::new(15.0, semibold()), Color32::WHITE);
        ui.label(RichText::new(title).size(19.0).family(semibold()));
    });
    if !sub.is_empty() {
        ui.label(RichText::new(sub).weak());
    }
    ui.add_space(6.0);
}

fn badge(ui: &mut Ui, text: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(13.0, semibold()), color);
    let size = galley.size() + vec2(20.0, 8.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 20.0, color.gamma_multiply(0.18));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
}

/// Path relative to `base` when inside it, otherwise shortened.
fn rel_or_short(p: &Path, base: Option<&Path>) -> String {
    if let Some(b) = base {
        if let Ok(r) = p.strip_prefix(b) {
            let s = r.to_string_lossy().to_string();
            return if s.is_empty() { "\\".into() } else { format!("…\\{s}") };
        }
    }
    short_path(p, 50)
}

fn kind_icon(k: Kind) -> &'static str {
    match k {
        Kind::Image => "🖼",
        Kind::Video => "🎬",
        Kind::Audio => "🎵",
        Kind::Document => "📄",
        Kind::Other => "📦",
    }
}

fn kind_chips(ui: &mut Ui, k: &mut KindFilter) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(t("what_files")).strong());
        for (flag, icon, key) in [
            (&mut k.images, "🖼", "type_images"),
            (&mut k.videos, "🎬", "type_videos"),
            (&mut k.audio, "🎵", "type_audio"),
            (&mut k.docs, "📄", "type_docs"),
            (&mut k.other, "📦", "type_other"),
        ] {
            if chip(ui, *flag, &format!("{icon} {}", t(key))).clicked() {
                *flag = !*flag;
                changed = true;
            }
        }
    });
    changed
}

fn thumb_cell(ui: &mut Ui, thumbs: &mut Thumbs, path: &Path, kind: Kind, size: f32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    ui.painter().rect_filled(rect, 8.0, ui.visuals().extreme_bg_color);
    if kind == Kind::Image && fsutil::can_thumb(path) {
        if let Some(tex) = thumbs.get(path) {
            let ts = tex.size_vec2();
            let scale = (size / ts.x).min(size / ts.y);
            let r = Rect::from_center_size(rect.center(), ts * scale);
            let id = tex.id();
            ui.painter().image(id, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            resp.on_hover_ui(|ui| {
                ui.image((id, ts * 2.0));
            });
            return;
        }
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        kind_icon(kind),
        FontId::proportional(size * 0.42),
        ui.visuals().weak_text_color(),
    );
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string_lossy().to_string())
}


/// The album / "check the date" / duplicates tick boxes (shared by Sync and Organize).
fn org_option_boxes(ui: &mut Ui, s: &mut Settings, with_dups: bool) -> bool {
    let mut changed = false;
    changed |= ui.checkbox(&mut s.org_keep_albums, t("opt_albums")).changed();
    ui.label(RichText::new(format!("      {}", t("opt_albums_hint"))).weak().small());
    changed |= ui.checkbox(&mut s.org_flag_dates, t("opt_check_date")).changed();
    ui.label(RichText::new(format!("      {}", t("opt_check_date_hint"))).weak().small());
    if with_dups {
        changed |= ui.checkbox(&mut s.org_remove_dups, t("opt_dups")).changed();
        ui.label(RichText::new(format!("      {}", t("opt_dups_hint"))).weak().small());
    }
    changed
}

fn ps_display_short(path: &[String]) -> String {
    path.join(" › ")
}

/// "C:\…\Laptop\Slike" for long paths.
pub fn short_path(p: &Path, max: usize) -> String {
    let full = p.to_string_lossy().to_string();
    if full.chars().count() <= max {
        return full;
    }
    let comps: Vec<String> = p.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    let head = comps.first().cloned().unwrap_or_default().trim_end_matches('\\').to_string();
    let mut tail: Vec<String> = Vec::new();
    for c in comps.iter().skip(1).rev() {
        let len: usize = tail.iter().map(|s| s.chars().count() + 1).sum::<usize>() + c.chars().count();
        if len + head.len() + 3 > max && !tail.is_empty() {
            break;
        }
        tail.insert(0, c.clone());
    }
    format!("{head}\\…\\{}", tail.join("\\"))
}

fn pick_folder(start: Option<&Path>) -> Option<PathBuf> {
    let mut d = rfd::FileDialog::new().set_title(t("pick_folder"));
    if let Some(s) = start {
        d = d.set_directory(s);
    }
    d.pick_folder()
}

fn drive_text(d: &Drive) -> String {
    let label = if d.label.is_empty() { t("local_disk").to_string() } else { d.label.clone() };
    let letter = d.root.to_string_lossy().trim_end_matches('\\').to_string();
    format!("{} {letter} {label} · {}", if d.removable { "🔌" } else { "💽" }, tf("free_of", &[&fmt_size(d.free), &fmt_size(d.total)]))
}

// ---------------------------------------------------------------------------
// eframe::App
// ---------------------------------------------------------------------------

impl eframe::App for MarkSync {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_theme(ctx);
        self.thumbs.poll(ctx);
        self.poll_job(ctx);
        if self.drives_time.elapsed() > Duration::from_secs(5) && self.job.is_none() {
            self.drives = fsutil::list_drives();
            self.drives_time = Instant::now();
        }
        self.handle_drop(ctx);
        self.demo_step(ctx);

        let panel = ctx.style().visuals.panel_fill;
        egui::TopBottomPanel::top("top")
            .frame(Frame::none().fill(panel).inner_margin(Margin::symmetric(22.0, 12.0)))
            .show_separator_line(false)
            .show(ctx, |ui| self.top_bar(ui));

        let busy = self.job.is_some() || self.confirm.is_some() || self.report.is_some() || self.phone_ui.is_some();
        if self.tab == Tab::Sync && self.compared {
            egui::TopBottomPanel::bottom("syncbar")
                .frame(Frame::none().fill(ctx.style().visuals.window_fill).inner_margin(Margin::symmetric(22.0, 12.0)))
                .show(ctx, |ui| {
                    ui.add_enabled_ui(!busy, |ui| self.sync_bottom(ui));
                });
        }
        if self.tab == Tab::Dupes && self.dup_searched && !self.groups.is_empty() {
            egui::TopBottomPanel::bottom("dupbar")
                .frame(Frame::none().fill(ctx.style().visuals.window_fill).inner_margin(Margin::symmetric(22.0, 12.0)))
                .show(ctx, |ui| {
                    ui.add_enabled_ui(!busy, |ui| self.dup_bottom(ui));
                });
        }

        egui::CentralPanel::default()
            .frame(Frame::none().fill(panel).inner_margin(Margin { left: 22.0, right: 22.0, top: 6.0, bottom: 14.0 }))
            .show(ctx, |ui| {
                ui.add_enabled_ui(!busy, |ui| match self.tab {
                    Tab::Sync => self.sync_tab(ui),
                    Tab::Organize => self.organize_tab(ui),
                    Tab::Dupes => self.dupes_tab(ui),
                    Tab::Settings => self.settings_tab(ui),
                });
            });

        self.phone_window(ctx);
        self.job_window(ctx);
        self.confirm_window(ctx);
        self.report_window(ctx);
        self.toast_ui(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save();
        fsutil::save_cache();
    }
}

impl MarkSync {
    /// Screenshot helper for development: MARKSYNC_DEMO="compare,show_all" etc.
    fn demo_step(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            return;
        }
        static STEPS: std::sync::OnceLock<Mutex<Vec<String>>> = std::sync::OnceLock::new();
        let steps = STEPS.get_or_init(|| {
            let v = std::env::var("MARKSYNC_DEMO").unwrap_or_default();
            Mutex::new(v.split(',').filter(|s| !s.is_empty()).rev().map(String::from).collect())
        });
        let Some(step) = steps.lock().unwrap().pop() else { return };
        match step.as_str() {
            "compare" => self.start_compare(ctx),
            "show_all" => {
                self.hidden_status.clear();
                self.view_dirty = true;
            }
            "org" => self.tab = Tab::Organize,
            "dup" => self.tab = Tab::Dupes,
            "settings" => self.tab = Tab::Settings,
            "plan" => self.start_plan(ctx),
            "find_dups" => {
                let roots = self.s.dup_roots.clone();
                let (k, h) = (self.s.dup_kinds, self.s.skip_hidden);
                self.spawn(ctx, "job_dupes", move |p| Done::Dupes(dupes::find(&roots, k, h, p)));
            }
            "confirm_copy" => self.confirm = Some(Confirm::Copy),
            "phone" => self.phone_ui = Some(PhoneBrowser::new(ctx)),
            "light" => self.s.theme = Theme::Light,
            s if s.starts_with("lang:") => i18n::set_lang(Lang::from_code(&s[5..])),
            _ => {}
        }
        ctx.request_repaint();
    }

    fn handle_drop(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if dropped.is_empty() || self.job.is_some() {
            return;
        }
        for p in dropped.into_iter().filter(|p| p.is_dir()) {
            match self.tab {
                Tab::Organize => self.s.org_root = Some(p),
                Tab::Dupes => {
                    if !self.s.dup_roots.contains(&p) {
                        self.s.dup_roots.push(p)
                    }
                }
                _ => self.add_source(p),
            }
        }
        self.save();
    }

    fn add_source(&mut self, p: PathBuf) {
        if self.trees.iter().any(|t| t.root == p) {
            return;
        }
        if self.s.target.as_deref() == Some(p.as_path()) {
            self.toast(t("err_same_folder"));
            return;
        }
        self.trees.push(FolderTree::new(p, Vec::new()));
        self.compared = false;
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            // logo
            let (r, _) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::hover());
            ui.painter().rect_filled(r, 11.0, ACCENT);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, "⇄", FontId::proportional(24.0), Color32::WHITE);
            ui.label(RichText::new("MarkSync").size(22.0).family(semibold()));
            ui.add_space(24.0);
            let enabled = self.job.is_none();
            ui.add_enabled_ui(enabled, |ui| {
                for (tab, icon, key) in [
                    (Tab::Sync, "⇄", "tab_sync"),
                    (Tab::Organize, "🗂", "tab_org"),
                    (Tab::Dupes, "👯", "tab_dup"),
                    (Tab::Settings, "⚙", "tab_settings"),
                ] {
                    let on = self.tab == tab;
                    let txt = RichText::new(format!("{icon}  {}", t(key))).size(15.5);
                    let txt = if on { txt.color(Color32::WHITE).family(semibold()) } else { txt };
                    let b = egui::Button::new(txt)
                        .rounding(10.0)
                        .min_size(vec2(0.0, 36.0))
                        .fill(if on { ACCENT } else { Color32::TRANSPARENT });
                    if ui.add(b).clicked() {
                        self.tab = tab;
                    }
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(RichText::new("☕").size(18.0)).rounding(10.0).min_size(vec2(36.0, 36.0)))
                    .on_hover_text(t("coffee_title"))
                    .clicked()
                {
                    self.tab = Tab::Settings;
                }
                let cur = i18n::lang();
                egui::ComboBox::from_id_salt("lang")
                    .selected_text(format!("🌐 {}", cur.native_name()))
                    .show_ui(ui, |ui| {
                        for l in Lang::ALL {
                            if ui.selectable_label(cur == l, l.native_name()).clicked() {
                                self.set_lang(l);
                            }
                        }
                    });
            });
        });
    }

    fn set_lang(&mut self, l: Lang) {
        i18n::set_lang(l);
        self.s.lang = Some(l.code().to_string());
        self.save();
    }

    // -----------------------------------------------------------------------
    // Sync
    // -----------------------------------------------------------------------

    fn sync_tab(&mut self, ui: &mut Ui) {
        if self.compared {
            self.sync_results(ui);
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.heading(t("sync_title"));
            ui.label(RichText::new(t("sync_sub")).weak().size(15.5));
            ui.add_space(12.0);
            ui.columns(2, |c| {
                card(&mut c[0], |ui| {
                    ui.set_width(ui.available_width());
                    self.source_card(ui)
                });
                card(&mut c[1], |ui| {
                    ui.set_width(ui.available_width());
                    self.target_card(ui)
                });
            });
            ui.add_space(12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                if kind_chips(ui, &mut self.s.kinds) {
                    self.save();
                }
            });
            ui.add_space(16.0);
            let ready = (!self.trees.is_empty() || !self.s.phone_sources.is_empty())
                && self.s.target.is_some()
                && self.s.kinds.any();
            ui.vertical_centered(|ui| {
                let r = big_button(ui, &format!("🔍  {}", t("compare")), ready);
                let r = if ready { r } else { r.on_disabled_hover_text(t("compare_need")) };
                if r.clicked() {
                    self.start_compare(ui.ctx());
                }
                ui.add_space(6.0);
                ui.label(RichText::new(t("safe_note")).weak());
            });
        });
    }

    fn source_card(&mut self, ui: &mut Ui) {
        section_title(ui, "1", t("src_title"), t("src_sub"));
        ui.horizontal_wrapped(|ui| {
            if ui.button(format!("➕ {}", t("add_folder"))).clicked() {
                if let Some(p) = pick_folder(None) {
                    self.add_source(p);
                    self.save();
                }
            }
            if ui.button(format!("📱 {}", t("phone_btn"))).on_hover_text(t("phone_btn_hint")).clicked() {
                self.phone_ui = Some(PhoneBrowser::new(ui.ctx()));
            }
            for (k, icon, key) in [
                (Known::Pictures, "🖼", "kf_pictures"),
                (Known::Videos, "🎬", "kf_videos"),
                (Known::Desktop, "🖥", "kf_desktop"),
                (Known::Downloads, "⬇", "kf_downloads"),
                (Known::Documents, "📄", "kf_documents"),
            ] {
                if let Some(p) = fsutil::known_folder(k) {
                    if !self.trees.iter().any(|t| t.root == p) && chip(ui, false, &format!("{icon} {}", t(key))).clicked() {
                        self.add_source(p);
                        self.save();
                    }
                }
            }
        });
        ui.add_space(6.0);
        let mut remove_phone = None;
        for (i, ps) in self.s.phone_sources.iter().enumerate() {
            Frame::none().fill(ui.visuals().faint_bg_color).rounding(10.0).inner_margin(8.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    ui.label(RichText::new(format!("📱 {}", ps.display())).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("✖").on_hover_text(t("remove")).clicked() {
                            remove_phone = Some(i);
                        }
                    });
                });
            });
        }
        if let Some(i) = remove_phone {
            self.s.phone_sources.remove(i);
            self.save();
        }
        if self.trees.is_empty() && self.s.phone_sources.is_empty() {
            Frame::none().fill(ui.visuals().faint_bg_color).rounding(10.0).inner_margin(14.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(t("src_empty")).weak());
            });
        }
        let mut remove = None;
        let mut changed = false;
        for (i, tr) in self.trees.iter_mut().enumerate() {
            Frame::none().fill(ui.visuals().faint_bg_color).rounding(10.0).inner_margin(8.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let before = tr.states.len();
                if tr.ui(ui, i) {
                    remove = Some(i);
                }
                if tr.states.len() != before {
                    changed = true;
                }
            });
        }
        if let Some(i) = remove {
            self.trees.remove(i);
            changed = true;
        }
        if changed {
            self.save();
        }
        if !self.trees.is_empty() {
            ui.label(RichText::new(t("src_tip")).weak().small());
        }
        ui.add_space(4.0);
        ui.label(RichText::new(t("phone_tip")).weak().small());
    }

    fn target_card(&mut self, ui: &mut Ui) {
        section_title(ui, "2", t("tgt_title"), t("tgt_sub"));
        ui.horizontal_wrapped(|ui| {
            if ui.button(format!("📁 {}", t("pick_folder"))).clicked() {
                if let Some(p) = pick_folder(self.s.target.as_deref()) {
                    if self.trees.iter().any(|t| t.root == p) {
                        self.toast(t("err_same_folder"));
                    } else {
                        self.s.target = Some(p);
                        self.save();
                    }
                }
            }
            if ui.button("⟳").on_hover_text(t("refresh_drives")).clicked() {
                self.drives = fsutil::list_drives();
            }
        });
        let drives: Vec<Drive> = self.drives.iter().filter(|d| !d.system).cloned().collect();
        if drives.is_empty() {
            ui.label(RichText::new(t("no_ext_drive")).weak().small());
        }
        for d in drives {
            let on = self.s.target.as_deref().map(|p| p.starts_with(&d.root)).unwrap_or(false);
            if chip(ui, on, &drive_text(&d)).clicked() {
                // a "MarkSync"-free default: the drive root, the user may pick a sub-folder
                self.s.target = Some(d.root.clone());
                self.save();
            }
        }
        ui.add_space(8.0);
        match self.s.target.clone() {
            Some(tg) => {
                Frame::none().fill(ui.visuals().faint_bg_color).rounding(10.0).inner_margin(12.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("📁").size(20.0));
                        ui.add(egui::Label::new(RichText::new(short_path(&tg, 60)).strong().size(16.0)).truncate())
                            .on_hover_text(tg.to_string_lossy());
                    });
                    if let Some(d) = self.drives.iter().find(|d| tg.starts_with(&d.root)) {
                        let used = 1.0 - d.free as f32 / d.total.max(1) as f32;
                        ui.add(egui::ProgressBar::new(used).desired_height(8.0).fill(if used > 0.92 { RED } else { ACCENT }));
                        ui.label(RichText::new(tf("free_of", &[&fmt_size(d.free), &fmt_size(d.total)])).weak().small());
                    } else if !tg.exists() {
                        ui.label(RichText::new(t("tgt_missing")).color(AMBER).small());
                    }
                });
            }
            None => {
                Frame::none().fill(ui.visuals().faint_bg_color).rounding(10.0).inner_margin(14.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(t("tgt_empty")).weak());
                });
            }
        }
        ui.add_space(10.0);
        ui.label(RichText::new(t("layout_q")).strong());
        let mut changed = false;
        changed |= ui.radio_value(&mut self.s.layout, LayoutChoice::Keep, t("layout_keep")).changed();
        changed |= ui.radio_value(&mut self.s.layout, LayoutChoice::Flat, t("layout_flat")).changed();
        ui.horizontal(|ui| {
            changed |= ui.radio_value(&mut self.s.layout, LayoutChoice::Organized, t("layout_org")).changed();
            ui.add_enabled_ui(self.s.layout == LayoutChoice::Organized, |ui| {
                egui::ComboBox::from_id_salt("layout_scheme")
                    .selected_text(format!("{} {}", self.s.layout_scheme.icon(), self.s.layout_scheme.title()))
                    .show_ui(ui, |ui| {
                        for sc in Scheme::ALL {
                            changed |= ui
                                .selectable_value(&mut self.s.layout_scheme, sc, format!("{} {}", sc.icon(), sc.title()))
                                .changed();
                        }
                    });
            });
        });
        if self.s.layout == LayoutChoice::Organized {
            ui.label(RichText::new(format!("{}  {}", t("example"), self.s.layout_scheme.example())).weak().small());
            changed |= org_option_boxes(ui, &mut self.s, false);
        }
        if changed {
            self.save();
        }
    }

    fn start_compare(&mut self, ctx: &egui::Context) {
        let Some(target) = self.s.target.clone() else { return };
        self.save();
        let inp = sync::CompareInput {
            sources: self.trees.iter().map(|t| t.source()).collect(),
            phones: self.s.phone_sources.clone(),
            target,
            kinds: self.s.kinds,
            layout: self.s.layout,
            scheme: self.s.layout_scheme,
            skip_hidden: self.s.skip_hidden,
            org_opts: organize::OrgOptions { remove_dups: false, ..self.s.org_options() },
        };
        if inp.layout == LayoutChoice::Organized && inp.scheme.needs_meta() {
            crate::meta::warm_up_geocoder();
        }
        self.spawn(ctx, "job_compare", move |p| Done::Compare(sync::compare(&inp, p)));
    }

    fn rebuild_view(&mut self) {
        let q = self.search.to_lowercase();
        self.view = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| !self.hidden_status.contains(&it.status))
            .filter(|(_, it)| q.is_empty() || it.src.to_string_lossy().to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
        self.view_dirty = false;
    }

    fn sync_results(&mut self, ui: &mut Ui) {
        if self.view_dirty {
            self.rebuild_view();
        }
        ui.horizontal(|ui| {
            if ui.button(format!("⬅ {}", t("back"))).clicked() {
                self.compared = false;
                self.items.clear();
            }
            ui.heading(t("results_title"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(format!("🔄 {}", t("compare_again"))).clicked() {
                    self.start_compare(ui.ctx());
                }
            });
        });
        ui.add_space(6.0);

        // summary cards
        let mut counts = [(0usize, 0u64); 5];
        for it in &self.items {
            let i = Status::ALL.iter().position(|s| *s == it.status).unwrap();
            counts[i].0 += 1;
            counts[i].1 += it.size;
        }
        ui.horizontal_wrapped(|ui| {
            for (i, st) in Status::ALL.iter().enumerate() {
                let (n, b) = counts[i];
                if n == 0 {
                    continue;
                }
                let visible = !self.hidden_status.contains(st);
                let col = status_color(*st);
                let resp = Frame::none()
                    .fill(if visible { col.gamma_multiply(0.16) } else { ui.visuals().window_fill })
                    .stroke(Stroke::new(1.5_f32, if visible { col } else { ui.visuals().widgets.noninteractive.bg_stroke.color }))
                    .rounding(12.0)
                    .inner_margin(Margin::symmetric(14.0, 8.0))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(fmt_num(n as u64)).size(22.0).family(semibold()).color(col));
                            ui.label(RichText::new(st.label()).strong());
                            ui.label(RichText::new(fmt_size(b)).weak().small());
                        });
                    })
                    .response
                    .interact(Sense::click())
                    .on_hover_text(format!("{}\n\n{}", st.hint(), t("click_to_toggle")));
                if resp.clicked() {
                    if visible {
                        self.hidden_status.insert(*st);
                    } else {
                        self.hidden_status.remove(st);
                    }
                    self.view_dirty = true;
                }
            }
        });
        if !self.warnings.is_empty() {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("⚠ {}", tf("warn_unreadable", &[&fmt_num(self.warnings.len() as u64)]))).color(AMBER).strong());
                ui.label(RichText::new(t("warn_unreadable_hint")).weak().small());
                egui::CollapsingHeader::new(t("warn_show")).id_salt("warns").show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                        for w in self.warnings.iter().take(500) {
                            ui.label(RichText::new(w).small());
                        }
                    });
                });
            });
            ui.add_space(6.0);
        }
        if self.items.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(t("nothing_found")).size(18.0)));
            return;
        }
        if counts[0].0 + counts[1].0 + counts[2].0 == 0 {
            ui.add_space(10.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("🎉  {}", t("all_there"))).size(18.0).color(GREEN));
            });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(format!("🔎 {}", t("search"))).desired_width(260.0));
            if r.changed() {
                self.view_dirty = true;
            }
            if ui.button(format!("☑ {}", t("select_all"))).clicked() {
                for &i in &self.view {
                    let it = &mut self.items[i];
                    if it.status.copyable() {
                        it.selected = true;
                    }
                }
            }
            if ui.button(format!("☐ {}", t("select_none"))).clicked() {
                for &i in &self.view {
                    self.items[i].selected = false;
                }
            }
            ui.label(RichText::new(tf("shown", &[&fmt_num(self.view.len() as u64)])).weak());
        });
        ui.add_space(4.0);

        let target = self.s.target.clone();
        let roots: Vec<PathBuf> = self.trees.iter().map(|t| t.root.clone()).collect();
        let Self { items, view, thumbs, .. } = self;
        let text_h = ui.text_style_height(&egui::TextStyle::Body);
        let src_rel = |p: &Path| -> String {
            let dir = p.parent().unwrap_or(p);
            match roots.iter().find(|r| dir.starts_with(r)) {
                Some(r) => {
                    let name = r.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| r.to_string_lossy().to_string());
                    let inner = dir.strip_prefix(r).unwrap().to_string_lossy().to_string();
                    if inner.is_empty() { name } else { format!("{name}\\{inner}") }
                }
                None => short_path(dir, 50),
            }
        };
        TableBuilder::new(ui)
            .striped(true)
            .auto_shrink([false, false])
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(30.0))
            .column(Column::exact(62.0))
            .column(Column::remainder().at_least(220.0).clip(true))
            .column(Column::exact(90.0))
            .column(Column::exact(140.0))
            .column(Column::exact(130.0))
            .column(Column::exact(170.0))
            .header(26.0, |mut h| {
                h.col(|_| {});
                h.col(|_| {});
                h.col(|ui| {
                    ui.label(RichText::new(t("col_file")).weak());
                });
                h.col(|ui| {
                    ui.label(RichText::new(t("col_size")).weak());
                });
                h.col(|ui| {
                    ui.label(RichText::new(t("col_date")).weak());
                });
                h.col(|ui| {
                    ui.label(RichText::new(t("col_status")).weak());
                });
                h.col(|ui| {
                    ui.label(RichText::new(t("col_action")).weak());
                });
            })
            .body(|body| {
                body.rows(60.0, view.len(), |mut row| {
                    let idx = view[row.index()];
                    let it = &mut items[idx];
                    row.col(|ui| {
                        ui.add_enabled(it.status.copyable(), egui::Checkbox::without_text(&mut it.selected));
                    });
                    row.col(|ui| thumb_cell(ui, thumbs, &it.src, it.kind, 52.0));
                    row.col(|ui| {
                        ui.vertical(|ui| {
                            ui.add_space((60.0 - text_h * 2.6).max(0.0) / 2.0);
                            ui.add(egui::Label::new(RichText::new(file_name(&it.src)).strong()).truncate());
                            let sub = match it.status {
                                Status::Exists => format!(
                                    "{}  ≡  {}",
                                    src_rel(&it.src),
                                    it.same_as.as_deref().map(|p| rel_or_short(p, target.as_deref())).unwrap_or_default()
                                ),
                                Status::DupInSource => format!(
                                    "{}  ≡  {}",
                                    src_rel(&it.src),
                                    it.same_as.as_deref().map(|p| src_rel(p) + "\\" + &file_name(p)).unwrap_or_default()
                                ),
                                _ => format!(
                                    "{}  →  {}",
                                    src_rel(&it.src),
                                    it.dest.parent().map(|d| rel_or_short(d, target.as_deref())).unwrap_or_default()
                                ),
                            };
                            ui.add(egui::Label::new(RichText::new(sub).weak().small()).truncate())
                                .on_hover_text(format!("{}\n→ {}", it.src.display(), it.dest.display()));
                        });
                    });
                    row.col(|ui| {
                        ui.label(fmt_size(it.size));
                    });
                    row.col(|ui| {
                        ui.label(fmt_date(it.mtime));
                    });
                    row.col(|ui| {
                        badge(ui, it.status.label(), status_color(it.status));
                    });
                    row.col(|ui| match it.status {
                        Status::Newer | Status::Conflict => {
                            egui::ComboBox::from_id_salt(("act", idx)).width(150.0).selected_text(it.action.label()).show_ui(
                                ui,
                                |ui| {
                                    ui.selectable_value(&mut it.action, Action::Replace, Action::Replace.label());
                                    ui.selectable_value(&mut it.action, Action::KeepBoth, Action::KeepBoth.label());
                                },
                            );
                            if let Some(dm) = it.dest_mtime {
                                ui.label("ⓘ").on_hover_text(tf("on_disk_date", &[&fmt_date(dm)]));
                            }
                        }
                        Status::New => {
                            let txt = if it.renamed { t("act_copy_renamed") } else { t("act_copy") };
                            ui.label(RichText::new(txt).weak());
                        }
                        _ => {
                            if ui.small_button("📂").on_hover_text(t("show_in_explorer")).clicked() {
                                if let Some(p) = &it.same_as {
                                    fsutil::open_in_explorer(p, true);
                                }
                            }
                        }
                    });
                });
            });
    }

    fn sync_bottom(&mut self, ui: &mut Ui) {
        let (n, bytes) = self
            .items
            .iter()
            .filter(|i| i.selected && i.status.copyable())
            .fold((0u64, 0u64), |(n, b), i| (n + 1, b + i.size));
        let free = self.s.target.as_deref().and_then(fsutil::free_space);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(tf("selected_sum", &[&fmt_num(n), &fmt_size(bytes)])).size(17.0).family(semibold()));
                if let Some(f) = free {
                    let enough = f > bytes + 50 * 1024 * 1024;
                    let txt = tf("free_on_target", &[&fmt_size(f)]);
                    ui.label(if enough { RichText::new(txt).weak() } else { RichText::new(format!("⚠ {txt} — {}", t("not_enough"))).color(RED) });
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if big_button(ui, &format!("⬇  {}", t("copy_selected")), n > 0).clicked() {
                    self.confirm = Some(Confirm::Copy);
                }
            });
        });
    }

    // -----------------------------------------------------------------------
    // Organize
    // -----------------------------------------------------------------------

    fn organize_tab(&mut self, ui: &mut Ui) {
        if self.org_plan.is_some() {
            self.organize_preview(ui);
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.heading(t("org_title"));
            ui.label(RichText::new(t("org_sub")).weak().size(15.5));
            ui.add_space(12.0);
            let mut changed = false;

            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, "1", t("org_folder"), t("org_folder_sub"));
                ui.horizontal_wrapped(|ui| {
                    if ui.button(format!("📁 {}", t("pick_folder"))).clicked() {
                        if let Some(p) = pick_folder(self.s.org_root.as_deref()) {
                            self.s.org_root = Some(p);
                            changed = true;
                        }
                    }
                    for d in self.drives.iter().filter(|d| !d.system) {
                        let on = self.s.org_root.as_deref() == Some(d.root.as_path());
                        if chip(ui, on, &drive_text(d)).clicked() {
                            self.s.org_root = Some(d.root.clone());
                            changed = true;
                        }
                    }
                    if let Some(tg) = self.s.target.clone() {
                        if self.s.org_root.as_ref() != Some(&tg) && chip(ui, false, &format!("⇄ {}", t("use_sync_target"))).clicked() {
                            self.s.org_root = Some(tg);
                            changed = true;
                        }
                    }
                });
                if let Some(r) = &self.s.org_root {
                    ui.label(RichText::new(format!("📁 {}", short_path(r, 90))).strong().size(16.0)).on_hover_text(r.to_string_lossy());
                }
                ui.add_space(6.0);
                changed |= kind_chips(ui, &mut self.s.org_kinds);
            });
            ui.add_space(12.0);

            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, "2", t("org_how"), t("org_how_sub"));
                let per_row = ((ui.available_width() / 230.0).floor() as usize).clamp(1, 4);
                let w = (ui.available_width() - (per_row as f32 - 1.0) * 10.0) / per_row as f32;
                for chunk in Scheme::ALL.chunks(per_row) {
                    ui.horizontal(|ui| {
                        for &sc in chunk {
                            let on = self.s.org_scheme == sc;
                            let resp = Frame::none()
                                .fill(if on { ACCENT.gamma_multiply(0.18) } else { ui.visuals().faint_bg_color })
                                .stroke(Stroke::new(1.5_f32, if on { ACCENT } else { ui.visuals().widgets.noninteractive.bg_stroke.color }))
                                .rounding(12.0)
                                .inner_margin(12.0)
                                .show(ui, |ui| {
                                    ui.set_width(w - 24.0);
                                    ui.set_height(64.0);
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(sc.icon()).size(26.0));
                                        ui.vertical(|ui| {
                                            ui.label(RichText::new(sc.title()).strong());
                                            ui.add(egui::Label::new(RichText::new(sc.example()).weak().small()).truncate());
                                        });
                                    });
                                })
                                .response
                                .interact(Sense::click());
                            if resp.clicked() {
                                self.s.org_scheme = sc;
                                changed = true;
                            }
                        }
                    });
                }
                if matches!(self.s.org_scheme, Scheme::Location | Scheme::YearLocation) {
                    ui.label(RichText::new(format!("🌍 {}", t("loc_note"))).weak().small());
                }
            });
            ui.add_space(12.0);

            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, "3", t("org_mode"), "");
                changed |= ui.radio_value(&mut self.s.org_mode, OrgMode::MoveInPlace, t("org_move")).changed();
                ui.label(RichText::new(format!("      {}", t("org_move_hint"))).weak().small());
                ui.horizontal(|ui| {
                    changed |= ui.radio_value(&mut self.s.org_mode, OrgMode::CopyTo, t("org_copy")).changed();
                    if self.s.org_mode == OrgMode::CopyTo {
                        if ui.button(format!("📁 {}", t("pick_folder"))).clicked() {
                            if let Some(p) = pick_folder(self.s.org_dest.as_deref()) {
                                self.s.org_dest = Some(p);
                                changed = true;
                            }
                        }
                        if let Some(d) = &self.s.org_dest {
                            ui.label(RichText::new(d.to_string_lossy()).strong());
                        }
                    }
                });
                ui.label(RichText::new(format!("      {}", t("org_copy_hint"))).weak().small());
            });
            ui.add_space(12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, "4", t("org_extra"), "");
                changed |= org_option_boxes(ui, &mut self.s, true);
            });
            if changed {
                self.save();
            }
            ui.add_space(16.0);
            let ready = self.s.org_root.is_some()
                && self.s.org_kinds.any()
                && (self.s.org_mode == OrgMode::MoveInPlace || self.s.org_dest.is_some());
            ui.vertical_centered(|ui| {
                if big_button(ui, &format!("👁  {}", t("org_preview")), ready).clicked() {
                    self.start_plan(ui.ctx());
                }
            });

            if let Some(j) = organize::latest_journal() {
                if let Some(journal) = organize::load_journal(&j) {
                    ui.add_space(16.0);
                    card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("↩").size(22.0));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(t("undo_title")).strong());
                                ui.label(
                                    RichText::new(tf(
                                        "undo_info",
                                        &[&journal.when, &fmt_num(journal.moves.len() as u64), &journal.root.to_string_lossy()],
                                    ))
                                    .weak()
                                    .small(),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button(format!("↩ {}", t("undo"))).clicked() {
                                    self.confirm = Some(Confirm::Undo);
                                }
                            });
                        });
                    });
                }
            }
        });
    }

    fn start_plan(&mut self, ctx: &egui::Context) {
        let Some(root) = self.s.org_root.clone() else { return };
        let dest = self.s.org_dest.clone();
        let (scheme, mode, kinds, hidden) = (self.s.org_scheme, self.s.org_mode, self.s.org_kinds, self.s.skip_hidden);
        let opts = self.s.org_options();
        if matches!(scheme, Scheme::Location | Scheme::YearLocation) {
            crate::meta::warm_up_geocoder();
        }
        self.spawn(ctx, "job_plan", move |p| {
            Done::Plan(organize::plan(&root, dest.as_deref(), scheme, mode, kinds, hidden, opts, p))
        });
    }

    fn organize_preview(&mut self, ui: &mut Ui) {
        let Some(plan) = &self.org_plan else { return };
        let mut back = false;
        ui.horizontal(|ui| {
            if ui.button(format!("⬅ {}", t("back"))).clicked() {
                back = true;
            }
            ui.heading(t("org_preview_title"));
        });
        ui.add_space(6.0);
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                let verb = if plan.mode == OrgMode::MoveInPlace { "org_will_move" } else { "org_will_copy" };
                badge(ui, &tf(verb, &[&fmt_num(plan.moves.len() as u64), &fmt_size(plan.total_bytes)]), ACCENT);
                if plan.in_place > 0 {
                    badge(ui, &tf("org_in_place", &[&fmt_num(plan.in_place as u64)]), GREEN);
                }
                badge(ui, &tf("org_folders", &[&fmt_num(plan.folders.len() as u64)]), PURPLE);
                if !plan.dups.is_empty() {
                    let b: u64 = plan.dups.iter().map(|d| d.size).sum();
                    badge(ui, &tf("org_dups", &[&fmt_num(plan.dups.len() as u64), &fmt_size(b)]), AMBER);
                }
                if plan.uncertain > 0 {
                    badge(ui, &tf("org_uncertain", &[&fmt_num(plan.uncertain as u64)]), RED);
                }
                ui.label(RichText::new(format!("{} {}", plan.scheme.icon(), plan.scheme.title())).strong());
            });
            ui.label(RichText::new(format!("📁 {}", plan.dest_root.display())).weak());
        });
        ui.add_space(8.0);
        let avail = ui.available_height() - 100.0;
        ui.columns(2, |cols| {
            card(&mut cols[0], |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(t("org_new_folders")).strong());
                egui::ScrollArea::vertical().id_salt("orgf").max_height(avail - 40.0).auto_shrink([false, false]).show(ui, |ui| {
                    for (name, n, b) in &plan.folders {
                        ui.horizontal(|ui| {
                            ui.label(format!("📁 {name}"));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(RichText::new(format!("{} · {}", fmt_num(*n as u64), fmt_size(*b))).weak());
                            });
                        });
                    }
                });
            });
            card(&mut cols[1], |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(t("org_examples")).strong());
                egui::ScrollArea::vertical().id_salt("orgm").max_height(avail - 40.0).auto_shrink([false, false]).show_rows(
                    ui,
                    40.0,
                    plan.moves.len(),
                    |ui, range| {
                        for m in &plan.moves[range] {
                            ui.vertical(|ui| {
                                let from = m.from.strip_prefix(&plan.root).unwrap_or(&m.from);
                                let to = m.to.strip_prefix(&plan.dest_root).unwrap_or(&m.to);
                                ui.add(egui::Label::new(RichText::new(from.to_string_lossy()).small()).truncate());
                                ui.add(egui::Label::new(RichText::new(format!("→ {}", to.to_string_lossy())).small().color(ACCENT)).truncate());
                            });
                        }
                    },
                );
            });
        });
        ui.add_space(8.0);
        let has = !plan.moves.is_empty();
        ui.vertical_centered(|ui| {
            if big_button(ui, &format!("✔  {}", t("org_do")), has).clicked() {
                self.confirm = Some(Confirm::Organize);
            }
            if !has {
                ui.label(RichText::new(t("org_nothing")).color(GREEN));
            }
        });
        if back {
            self.org_plan = None;
        }
    }

    // -----------------------------------------------------------------------
    // Duplicates
    // -----------------------------------------------------------------------

    fn rebuild_dup_rows(&mut self) {
        self.dup_rows.clear();
        for (g, grp) in self.groups.iter().enumerate() {
            self.dup_rows.push((g, None));
            for f in 0..grp.files.len() {
                self.dup_rows.push((g, Some(f)));
            }
        }
    }

    fn dupes_tab(&mut self, ui: &mut Ui) {
        if self.dup_searched {
            self.dupes_results(ui);
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.heading(t("dup_title"));
            ui.label(RichText::new(t("dup_sub")).weak().size(15.5));
            ui.add_space(12.0);
            let mut changed = false;
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, "1", t("dup_where"), t("dup_where_sub"));
                ui.horizontal_wrapped(|ui| {
                    if ui.button(format!("➕ {}", t("add_folder"))).clicked() {
                        if let Some(p) = pick_folder(None) {
                            if !self.s.dup_roots.contains(&p) {
                                self.s.dup_roots.push(p);
                                changed = true;
                            }
                        }
                    }
                    let mut quick: Vec<(String, PathBuf)> = Vec::new();
                    if let Some(p) = fsutil::known_folder(Known::Pictures) {
                        quick.push((format!("🖼 {}", t("kf_pictures")), p));
                    }
                    for d in self.drives.iter().filter(|d| !d.system) {
                        quick.push((drive_text(d), d.root.clone()));
                    }
                    for (label, p) in quick {
                        if !self.s.dup_roots.contains(&p) && chip(ui, false, &label).clicked() {
                            self.s.dup_roots.push(p);
                            changed = true;
                        }
                    }
                });
                let mut remove = None;
                for (i, r) in self.s.dup_roots.iter().enumerate() {
                    Frame::none().fill(ui.visuals().faint_bg_color).rounding(10.0).inner_margin(8.0).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("📁 {}", short_path(r, 90))).strong()).on_hover_text(r.to_string_lossy());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("✖").on_hover_text(t("remove")).clicked() {
                                    remove = Some(i);
                                }
                            });
                        });
                    });
                }
                if let Some(i) = remove {
                    self.s.dup_roots.remove(i);
                    changed = true;
                }
                if self.s.dup_roots.is_empty() {
                    ui.label(RichText::new(t("src_empty")).weak());
                }
                ui.add_space(6.0);
                changed |= kind_chips(ui, &mut self.s.dup_kinds);
            });
            if changed {
                self.save();
            }
            ui.add_space(16.0);
            let ready = !self.s.dup_roots.is_empty() && self.s.dup_kinds.any();
            ui.vertical_centered(|ui| {
                if big_button(ui, &format!("🔍  {}", t("dup_find")), ready).clicked() {
                    let roots = self.s.dup_roots.clone();
                    let (k, h) = (self.s.dup_kinds, self.s.skip_hidden);
                    self.spawn(ui.ctx(), "job_dupes", move |p| Done::Dupes(dupes::find(&roots, k, h, p)));
                }
                ui.add_space(6.0);
                ui.label(RichText::new(t("dup_note")).weak());
            });
        });
    }

    fn dupes_results(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui.button(format!("⬅ {}", t("back"))).clicked() {
                self.dup_searched = false;
                self.groups.clear();
                self.dup_rows.clear();
            }
            ui.heading(t("dup_results"));
        });
        if self.groups.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(format!("🎉  {}", t("dup_none"))).size(18.0).color(GREEN)));
            return;
        }
        let extra: usize = self.groups.iter().map(|g| g.files.len() - 1).sum();
        let wasted: u64 = self.groups.iter().map(|g| g.wasted()).sum();
        ui.horizontal_wrapped(|ui| {
            badge(ui, &tf("dup_groups", &[&fmt_num(self.groups.len() as u64)]), ACCENT);
            badge(ui, &tf("dup_extra", &[&fmt_num(extra as u64)]), AMBER);
            badge(ui, &tf("dup_wasted", &[&fmt_size(wasted)]), GREEN);
        });
        ui.label(RichText::new(t("dup_keep_hint")).weak().small());
        ui.add_space(4.0);

        let roots = self.s.dup_roots.clone();
        let Self { groups, dup_rows, thumbs, .. } = self;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 34.0, dup_rows.len(), |ui, range| {
            for &(g, f) in &dup_rows[range] {
                let grp = &mut groups[g];
                ui.allocate_ui(vec2(ui.available_width(), 34.0), |ui| {
                    ui.set_height(34.0);
                    ui.horizontal_centered(|ui| match f {
                        None => {
                            ui.add_space(2.0);
                            let keep = grp.files[grp.keep].path.clone();
                            thumb_cell(ui, thumbs, &keep, grp.kind, 30.0);
                            ui.label(
                                RichText::new(tf("dup_group_head", &[&fmt_num(grp.files.len() as u64), &fmt_size(grp.size)]))
                                    .family(semibold()),
                            );
                        }
                        Some(fi) => {
                            let path = grp.files[fi].path.clone();
                            let mtime = grp.files[fi].mtime;
                            let root = roots.iter().find(|r| path.starts_with(r)).map(PathBuf::as_path);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("📂").on_hover_text(t("show_in_explorer")).clicked() {
                                    fsutil::open_in_explorer(&path, true);
                                }
                                ui.label(RichText::new(fmt_date(mtime)).weak().small());
                                if grp.keep == fi {
                                    badge(ui, t("dup_keep"), GREEN);
                                }
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.add_space(40.0);
                                    let mut keep = grp.keep;
                                    ui.radio_value(&mut keep, fi, "");
                                    grp.keep = keep;
                                    let txt = RichText::new(rel_or_short(&path, root));
                                    let txt = if grp.keep == fi { txt.strong() } else { txt.weak().strikethrough() };
                                    ui.add(egui::Label::new(txt).truncate()).on_hover_text(path.to_string_lossy());
                                });
                            });
                        }
                    });
                });
            }
        });
    }

    fn dup_bottom(&mut self, ui: &mut Ui) {
        let extra: u64 = self.groups.iter().map(|g| g.files.len() as u64 - 1).sum();
        let wasted: u64 = self.groups.iter().map(|g| g.wasted()).sum();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t("dup_what_extra")).strong());
            egui::ComboBox::from_id_salt("dupact")
                .selected_text(match self.s.dup_action {
                    DupAction::RecycleBin => t("dup_to_bin"),
                    DupAction::MoveToFolder => t("dup_to_folder"),
                })
                .width(300.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.s.dup_action, DupAction::RecycleBin, t("dup_to_bin"));
                    ui.selectable_value(&mut self.s.dup_action, DupAction::MoveToFolder, t("dup_to_folder"));
                });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if big_button(ui, &format!("🗑  {}", tf("dup_remove", &[&fmt_num(extra), &fmt_size(wasted)])), extra > 0).clicked() {
                    self.confirm = Some(Confirm::RemoveDups);
                }
            });
        });
    }

    // -----------------------------------------------------------------------
    // Settings
    // -----------------------------------------------------------------------

    fn settings_tab(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.heading(t("tab_settings"));
            ui.add_space(12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("☕  {}", t("coffee_title"))).size(18.0).family(semibold()));
                ui.label(t("coffee_text"));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    for (eur, cups) in [(3u32, "☕"), (5, "☕☕"), (10, "☕☕☕")] {
                        let b = egui::Button::new(RichText::new(format!("{cups}  {eur} €")).strong().color(Color32::WHITE))
                            .fill(Color32::from_rgb(0, 112, 186))
                            .rounding(10.0)
                            .min_size(vec2(0.0, 36.0));
                        if ui.add(b).on_hover_text(t("coffee_hover")).clicked() {
                            fsutil::open_url(&coffee_url(eur));
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("PayPal: {DONATE_EMAIL}")).weak());
                    if ui.small_button(t("copy")).clicked() {
                        ui.ctx().copy_text(DONATE_EMAIL.into());
                        self.toast(t("copied"));
                    }
                });
            });
            ui.add_space(12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("🌐 {}", t("language"))).size(18.0).family(semibold()));
                let cur = i18n::lang();
                for l in Lang::ALL {
                    if ui.radio(cur == l, l.native_name()).clicked() {
                        self.set_lang(l);
                    }
                }
            });
            ui.add_space(12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("🎨 {}", t("theme"))).size(18.0).family(semibold()));
                ui.horizontal(|ui| {
                    let mut th = self.s.theme;
                    if chip(ui, th == Theme::Dark, &format!("🌙 {}", t("theme_dark"))).clicked() {
                        th = Theme::Dark;
                    }
                    if chip(ui, th == Theme::Light, &format!("☀ {}", t("theme_light"))).clicked() {
                        th = Theme::Light;
                    }
                    if th != self.s.theme {
                        self.s.theme = th;
                        self.save();
                    }
                });
            });
            ui.add_space(12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("🛠 {}", t("advanced"))).size(18.0).family(semibold()));
                if ui.checkbox(&mut self.s.skip_hidden, t("skip_hidden")).changed() {
                    self.save();
                }
                ui.horizontal(|ui| {
                    if ui.button(format!("🧹 {}", t("clear_cache"))).clicked() {
                        fsutil::clear_cache();
                        self.toast(t("cache_cleared"));
                    }
                    ui.label(RichText::new(t("clear_cache_hint")).weak().small());
                });
                ui.horizontal(|ui| {
                    if ui.button(format!("📜 {}", t("history"))).clicked() {
                        let _ = std::fs::create_dir_all(fsutil::history_dir());
                        fsutil::open_in_explorer(&fsutil::history_dir(), false);
                    }
                    ui.label(RichText::new(t("history_hint")).weak().small());
                });
                if ui.button(format!("📂 {}", t("open_data"))).clicked() {
                    let _ = std::fs::create_dir_all(crate::settings::data_dir());
                    fsutil::open_in_explorer(&crate::settings::data_dir(), false);
                }
            });
            ui.add_space(12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("ℹ {}", t("about"))).size(18.0).family(semibold()));
                ui.label(format!("MarkSync {}", env!("CARGO_PKG_VERSION")));
                ui.label(tf("made_by", &[AUTHOR]));
                ui.label(RichText::new(t("made_with_claude")).weak());
                ui.label(RichText::new(t("about_text")).weak());
            });
        });
    }

    // -----------------------------------------------------------------------
    // Overlays
    // -----------------------------------------------------------------------

    fn phone_window(&mut self, ctx: &egui::Context) {
        let Some(b) = &mut self.phone_ui else { return };
        b.poll();
        let mut close = false;
        let mut chosen: Option<phone::PhoneSource> = None;
        let mut open = true;
        egui::Window::new(format!("📱 {}", t("phone_title")))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .order(egui::Order::Foreground)
            .fixed_size(vec2(560.0, 0.0))
            .show(ctx, |ui| {
                // 1. which phone
                let Some(devs) = b.devices.clone() else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t("phone_searching"));
                    });
                    return;
                };
                if b.dev.is_none() {
                    if devs.is_empty() {
                        ui.label(RichText::new(t("phone_none")).size(15.5));
                        ui.add_space(6.0);
                        if ui.button(format!("⟳ {}", t("phone_retry"))).clicked() {
                            b.load_devices(ctx);
                        }
                        return;
                    }
                    if devs.len() == 1 {
                        b.open_device(ctx, devs[0].clone());
                    } else {
                        ui.label(t("phone_which"));
                        for d in devs {
                            if chip(ui, false, &format!("📱 {}", d.name)).clicked() {
                                b.open_device(ctx, d);
                            }
                        }
                        return;
                    }
                }
                let Some(dev) = b.dev.clone() else { return };

                // 2. shortcut to the camera folder
                match &b.camera {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(t("phone_find_camera"));
                        });
                    }
                    Some(Some(path)) => {
                        let ps = phone::PhoneSource { device: dev.name.clone(), path: path.clone() };
                        if big_button(ui, &format!("📷  {}", t("phone_camera")), true).on_hover_text(ps.display()).clicked() {
                            chosen = Some(ps);
                        }
                        ui.label(RichText::new(ps_display_short(path)).weak().small());
                    }
                    Some(None) => {}
                }
                ui.add_space(8.0);
                ui.separator();
                ui.label(RichText::new(t("phone_or_pick")).strong());

                // 3. browse
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button(format!("📱 {}", dev.name)).clicked() {
                        b.stack.clear();
                        b.load_folders(ctx);
                    }
                    let mut cut = None;
                    for (i, (_, name)) in b.stack.iter().enumerate() {
                        ui.label("›");
                        if ui.small_button(name).clicked() {
                            cut = Some(i + 1);
                        }
                    }
                    if let Some(n) = cut {
                        b.stack.truncate(n);
                        b.load_folders(ctx);
                    }
                });
                let mut enter = None;
                egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| match &b.folders {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(t("phone_loading"));
                        });
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(RED));
                        ui.label(RichText::new(t("phone_unlock_hint")).weak());
                    }
                    Some(Ok(list)) if list.is_empty() => {
                        ui.label(RichText::new(t("phone_empty")).weak());
                        if b.stack.is_empty() {
                            ui.label(RichText::new(t("phone_unlock_hint")).weak());
                        }
                    }
                    Some(Ok(list)) => {
                        for e in list {
                            ui.horizontal(|ui| {
                                if ui.add(egui::Button::new(format!("📁 {}", e.name)).frame(false)).clicked() {
                                    enter = Some((e.obj.clone(), e.name.clone()));
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if !b.stack.is_empty() && ui.small_button(t("phone_pick")).clicked() {
                                        let mut path: Vec<String> = b.stack.iter().map(|s| s.1.clone()).collect();
                                        path.push(e.name.clone());
                                        chosen = Some(phone::PhoneSource { device: dev.name.clone(), path });
                                    }
                                });
                            });
                        }
                    }
                });
                if let Some(en) = enter {
                    b.stack.push(en);
                    b.load_folders(ctx);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if b.stack.len() >= 2 && ui.button(format!("✔ {}", t("phone_pick_this"))).clicked() {
                        chosen = Some(phone::PhoneSource { device: dev.name.clone(), path: b.stack.iter().map(|s| s.1.clone()).collect() });
                    }
                    if ui.button(t("cancel")).clicked() {
                        close = true;
                    }
                });
            });
        if let Some(ps) = chosen {
            if !self.s.phone_sources.contains(&ps) {
                self.s.phone_sources.push(ps);
            }
            self.compared = false;
            self.save();
            close = true;
        }
        if close || !open {
            self.phone_ui = None;
        }
    }

    fn job_window(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let p = job.p.clone();
        let title = job.title;
        let elapsed = job.started.elapsed();
        egui::Window::new(t(title))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .order(egui::Order::Foreground)
            .fixed_size(vec2(520.0, 0.0))
            .show(ctx, |ui| {
                ui.add_space(4.0);
                let stage = *p.stage.lock().unwrap();
                ui.label(RichText::new(t(stage)).size(16.0).family(semibold()));
                let done = p.done.load(std::sync::atomic::Ordering::Relaxed);
                let total = p.total.load(std::sync::atomic::Ordering::Relaxed);
                match p.fraction() {
                    Some(f) => {
                        ui.add(egui::ProgressBar::new(f).show_percentage().desired_height(14.0).fill(ACCENT));
                    }
                    None => {
                        ui.add(egui::ProgressBar::new(0.0).animate(true).desired_height(14.0).fill(ACCENT));
                    }
                }
                let bt = p.bytes_total.load(std::sync::atomic::Ordering::Relaxed);
                let mut info = if total > 0 {
                    format!("{} / {}", fmt_num(done), fmt_num(total))
                } else {
                    tf("found_n", &[&fmt_num(done)])
                };
                if bt > 0 {
                    info.push_str(&format!("  ·  {} / {}", fmt_size(p.bytes.load(std::sync::atomic::Ordering::Relaxed)), fmt_size(bt)));
                }
                info.push_str(&format!("  ·  {}:{:02}", elapsed.as_secs() / 60, elapsed.as_secs() % 60));
                ui.label(RichText::new(info).weak());
                let cur = p.current.lock().unwrap().clone();
                ui.add(egui::Label::new(RichText::new(cur).small().weak()).truncate());
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    if p.cancelled() {
                        ui.label(t("cancelling"));
                    } else if ui.button(format!("✖ {}", t("cancel"))).clicked() {
                        p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            });
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(c) = self.confirm else { return };
        let (title, text) = match c {
            Confirm::Copy => {
                let (n, b) = self
                    .items
                    .iter()
                    .filter(|i| i.selected && i.status.copyable())
                    .fold((0u64, 0u64), |(n, b), i| (n + 1, b + i.size));
                let repl = self.items.iter().filter(|i| i.selected && i.status.copyable() && i.action == Action::Replace).count();
                let mut s = tf("confirm_copy", &[&fmt_num(n), &fmt_size(b), &self.s.target.as_deref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()]);
                if repl > 0 {
                    s.push_str("\n\n");
                    s.push_str(&tf("confirm_replace", &[&fmt_num(repl as u64)]));
                }
                (t("confirm_title"), s)
            }
            Confirm::Organize => {
                let plan = self.org_plan.as_ref();
                let n = plan.map(|p| p.moves.len()).unwrap_or(0) as u64;
                let in_place = plan.map(|p| p.mode) == Some(OrgMode::MoveInPlace);
                let key = if in_place { "confirm_org_move" } else { "confirm_org_copy" };
                let mut s = tf(key, &[&fmt_num(n)]);
                let d = plan.map(|p| p.dups.len()).unwrap_or(0) as u64;
                if d > 0 {
                    s.push_str("\n\n");
                    s.push_str(&tf(if in_place { "confirm_org_dups" } else { "confirm_org_dups_copy" }, &[&fmt_num(d)]));
                }
                (t("confirm_title"), s)
            }
            Confirm::RemoveDups => {
                let extra: u64 = self.groups.iter().map(|g| g.files.len() as u64 - 1).sum();
                let key = if self.s.dup_action == DupAction::RecycleBin { "confirm_dup_bin" } else { "confirm_dup_folder" };
                (t("confirm_title"), tf(key, &[&fmt_num(extra)]))
            }
            Confirm::Undo => (t("confirm_title"), t("confirm_undo").to_string()),
        };
        let mut close = false;
        let mut go = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .order(egui::Order::Foreground)
            .fixed_size(vec2(480.0, 0.0))
            .show(ctx, |ui| {
                ui.label(RichText::new(text).size(15.5));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if big_button(ui, &format!("✔ {}", t("yes_go")), true).clicked() {
                        go = true;
                    }
                    if ui.add(egui::Button::new(t("cancel")).min_size(vec2(120.0, 46.0)).rounding(12.0)).clicked() {
                        close = true;
                    }
                });
            });
        if close {
            self.confirm = None;
        }
        if go {
            self.confirm = None;
            match c {
                Confirm::Copy => {
                    let items = self.items.clone();
                    self.spawn(ctx, "job_copy", move |p| Done::Copy(sync::copy(&items, p)));
                }
                Confirm::Organize => {
                    if let Some(plan) = self.org_plan.take() {
                        self.spawn(ctx, "job_org", move |p| {
                            let (rep, j) = organize::execute(&plan, p);
                            Done::Organized(rep, j)
                        });
                    }
                }
                Confirm::RemoveDups => {
                    let groups = self.groups.clone();
                    let roots = self.s.dup_roots.clone();
                    let action = self.s.dup_action;
                    self.save();
                    self.spawn(ctx, "job_dup_remove", move |p| Done::DupRemoved(dupes::remove_extras(&groups, &roots, action, p)));
                }
                Confirm::Undo => {
                    if let Some(j) = organize::latest_journal() {
                        self.spawn(ctx, "job_undo", move |p| Done::Undo(organize::undo(&j, p)));
                    }
                }
            }
        }
    }

    fn report_window(&mut self, ctx: &egui::Context) {
        let Some((title, rep)) = &self.report else { return };
        let mut close = false;
        let mut organize_target = false;
        let offer_organize = *title == "rep_copy"
            && rep.ok_files > 0
            && !rep.cancelled
            && self.s.layout == LayoutChoice::Organized
            && self.s.target.is_some();
        egui::Window::new(t(title))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .order(egui::Order::Foreground)
            .fixed_size(vec2(520.0, 0.0))
            .show(ctx, |ui| {
                let is_error_only = *title == "err_title";
                if !is_error_only {
                    if rep.cancelled {
                        ui.label(RichText::new(format!("⏹ {}", t("was_cancelled"))).color(AMBER).size(16.0));
                    } else if rep.errors.is_empty() {
                        ui.label(RichText::new(format!("🎉 {}", t("all_done"))).color(GREEN).size(18.0).family(semibold()));
                    }
                    ui.label(RichText::new(tf("rep_ok", &[&fmt_num(rep.ok_files), &fmt_size(rep.ok_bytes)])).size(16.0));
                    if rep.dups_removed > 0 {
                        ui.label(tf("rep_dups_removed", &[&fmt_num(rep.dups_removed)]));
                    }
                }
                if !rep.errors.is_empty() {
                    if !is_error_only {
                        ui.label(RichText::new(tf("rep_errors", &[&fmt_num(rep.errors.len() as u64)])).color(RED));
                    }
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        for (p, e) in rep.errors.iter().take(300) {
                            if !p.is_empty() {
                                ui.label(RichText::new(p).small().strong());
                            }
                            ui.label(RichText::new(e).small().color(RED));
                        }
                    });
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("OK").color(Color32::WHITE)).fill(ACCENT).min_size(vec2(120.0, 40.0)).rounding(10.0)).clicked() {
                        close = true;
                    }
                    if let Some(p) = &rep.open_path {
                        if ui.add(egui::Button::new(format!("📂 {}", t("open_folder"))).min_size(vec2(0.0, 40.0)).rounding(10.0)).clicked() {
                            fsutil::open_in_explorer(p, false);
                        }
                    }
                });
                if offer_organize {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(RichText::new(t("after_copy_q")).strong());
                    ui.label(RichText::new(t("after_copy_hint")).weak().small());
                    if big_button(ui, &format!("🗂  {}", t("after_copy_btn")), true).clicked() {
                        organize_target = true;
                        close = true;
                    }
                }
            });
        if organize_target {
            if let Some(tg) = self.s.target.clone() {
                self.s.org_root = Some(tg);
                self.s.org_scheme = self.s.layout_scheme;
                self.s.org_mode = OrgMode::MoveInPlace;
                self.s.org_kinds = self.s.kinds;
                self.save();
                self.tab = Tab::Organize;
                self.start_plan(ctx);
            }
        }
        if close {
            self.report = None;
        }
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some((msg, at)) = &self.toast else { return };
        if at.elapsed() > Duration::from_secs(3) {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        egui::Area::new(egui::Id::new("toast"))
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -30.0))
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                Frame::none().fill(ACCENT).rounding(12.0).inner_margin(Margin::symmetric(18.0, 10.0)).show(ui, |ui| {
                    ui.label(RichText::new(msg).color(Color32::WHITE).size(15.0));
                });
            });
    }
}
