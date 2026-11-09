#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod dupes;
mod fsutil;
mod i18n;
mod job;
mod meta;
mod organize;
mod settings;
mod sync;
#[cfg(test)]
mod tests;
mod thumbs;
mod tree;

use eframe::egui;

pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let load = |name: &str| std::fs::read(format!(r"{windir}\Fonts\{name}")).ok();
    if let Some(data) = load("segoeui.ttf") {
        fonts.font_data.insert("segoe".into(), egui::FontData::from_owned(data));
        fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0, "segoe".into());
    }
    // Windows' own (monochrome) emoji outlines look cleaner than the built-in ones
    if let Some(data) = load("seguiemj.ttf") {
        fonts.font_data.insert("segoe_emoji".into(), egui::FontData::from_owned(data));
        let fam = fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap();
        let pos = if fam.first().map(|s| s == "segoe").unwrap_or(false) { 1 } else { 0 };
        fam.insert(pos, "segoe_emoji".into());
    }
    let mut semibold = Vec::new();
    if let Some(data) = load("seguisb.ttf").or_else(|| load("segoeuib.ttf")) {
        fonts.font_data.insert("segoe_sb".into(), egui::FontData::from_owned(data));
        semibold.push("segoe_sb".to_string());
    }
    // symbols that Segoe UI lacks (arrows, ticks) and the built-in emoji fonts
    if let Some(data) = load("seguisym.ttf") {
        fonts.font_data.insert("segoe_sym".into(), egui::FontData::from_owned(data));
        fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().push("segoe_sym".into());
    }
    let prop = fonts.families[&egui::FontFamily::Proportional].clone();
    semibold.extend(prop);
    fonts.families.insert(egui::FontFamily::Name("semibold".into()), semibold);
    ctx.set_fonts(fonts);
}

fn icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../assets/icon256.png")).expect("icon").to_rgba8();
    egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() }
}

fn main() -> eframe::Result<()> {
    fsutil::init_error_mode();
    let _ = rayon::ThreadPoolBuilder::new().num_threads(4).build_global();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MarkSync")
            .with_inner_size([1240.0, 820.0])
            .with_min_inner_size([960.0, 640.0])
            .with_icon(icon())
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("MarkSync", options, Box::new(|cc| Ok(Box::new(app::MarkSync::new(cc)))))
}
