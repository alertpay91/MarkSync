//! Folder tree with tick boxes (lazy loading of sub-folders).

use crate::fsutil;
use crate::sync::Source;
use eframe::egui::{self, RichText};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub struct FolderTree {
    pub root: PathBuf,
    pub states: HashMap<PathBuf, bool>,
    expanded: HashSet<PathBuf>,
    children: HashMap<PathBuf, Vec<PathBuf>>,
}

impl FolderTree {
    pub fn new(root: PathBuf, states: Vec<(PathBuf, bool)>) -> Self {
        Self { root, states: states.into_iter().collect(), expanded: HashSet::new(), children: HashMap::new() }
    }

    pub fn source(&self) -> Source {
        Source { root: self.root.clone(), states: self.states.clone() }
    }

    pub fn states_vec(&self) -> Vec<(PathBuf, bool)> {
        self.states.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }

    fn included(&self, p: &Path) -> bool {
        self.source().included(p)
    }

    fn mixed(&self, p: &Path, v: bool) -> bool {
        self.states.iter().any(|(k, &s)| k != p && k.starts_with(p) && s != v)
    }

    fn set(&mut self, p: &Path, v: bool) {
        self.states.retain(|k, _| !k.starts_with(p));
        // store only when it differs from the parent (keeps the map small)
        let parent_v = if p == self.root { true } else { p.parent().map(|pp| self.included(pp)).unwrap_or(true) };
        if v != parent_v || p == self.root {
            self.states.insert(p.to_path_buf(), v);
        }
    }

    fn kids(&mut self, p: &Path) -> Vec<PathBuf> {
        self.children.entry(p.to_path_buf()).or_insert_with(|| fsutil::subdirs(p)).clone()
    }

    /// Returns true when the user asked to remove this source.
    pub fn ui(&mut self, ui: &mut egui::Ui, id: usize) -> bool {
        let mut remove = false;
        let root = self.root.clone();
        ui.horizontal(|ui| {
            self.row(ui, &root, true);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("✖").on_hover_text(crate::i18n::t("remove")).clicked() {
                    remove = true;
                }
            });
        });
        if self.expanded.contains(&root) {
            egui::ScrollArea::vertical()
                .id_salt(("tree", id))
                .max_height(220.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.indent(("ind", id), |ui| {
                        let kids = self.kids(&root);
                        if kids.is_empty() {
                            ui.label(RichText::new(crate::i18n::t("no_subfolders")).weak().italics());
                        }
                        for k in kids {
                            self.node(ui, &k);
                        }
                    });
                });
        }
        remove
    }

    fn node(&mut self, ui: &mut egui::Ui, p: &Path) {
        ui.horizontal(|ui| self.row(ui, p, false));
        if self.expanded.contains(p) {
            ui.indent(p.to_string_lossy().to_string(), |ui| {
                for k in self.kids(p) {
                    self.node(ui, &k);
                }
            });
        }
    }

    fn row(&mut self, ui: &mut egui::Ui, p: &Path, is_root: bool) {
        let has_kids = !self.kids(p).is_empty();
        let open = self.expanded.contains(p);
        if has_kids {
            let arrow = if open { "⏷" } else { "⏵" };
            if ui.add(egui::Button::new(arrow).frame(false)).clicked() {
                if open {
                    self.expanded.remove(p);
                } else {
                    self.expanded.insert(p.to_path_buf());
                }
            }
        } else {
            ui.add_space(18.0);
        }
        let mut v = self.included(p);
        let mixed = self.mixed(p, v);
        let name = if is_root {
            let room = ((ui.available_width() - 70.0) / 8.0).max(20.0) as usize;
            crate::app::short_path(p, room)
        } else {
            p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        };
        let text = if is_root { RichText::new(format!("📁 {name}")).strong() } else { RichText::new(format!("📁 {name}")) };
        let r = ui.add(egui::Checkbox::new(&mut v, text).indeterminate(mixed));
        if r.changed() {
            self.set(p, v);
        }
        if is_root {
            r.on_hover_text(p.to_string_lossy());
        }
    }
}
