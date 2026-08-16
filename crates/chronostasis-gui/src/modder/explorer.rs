use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, OnceLock};

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

use super::ext_lower;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Textures,
    Audio,
    Text,
    Data,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Texture,
    Audio,
    Text,
    Data,
    Other,
}

pub fn file_kind(path: &Path) -> FileKind {
    match ext_lower(path).as_deref() {
        Some(
            "trb" | "xgr" | "imgb" | "wpd" | "dds" | "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tga"
            | "webp",
        ) => FileKind::Texture,
        Some("scd") => FileKind::Audio,
        Some("ztr" | "txt" | "xml") => FileKind::Text,
        Some("wdb") => FileKind::Data,
        _ => FileKind::Other,
    }
}

struct Node {
    path: PathBuf,
    name: String,
    is_dir: bool,
    /// `None` until first expanded.
    children: Option<Vec<Node>>,
}

#[derive(Default)]
pub struct Explorer {
    root: Option<PathBuf>,
    tree: Option<Node>,
    filter: Filter,
    name_filter: String,
    selected: Option<PathBuf>,
    grid: DirGrid,
    filter_gen: u64,
    /// Used to detect a filter change.
    filter_key: (Filter, String),
    /// Built once, so filtering is an in-memory match rather than a disk walk.
    file_index: Vec<PathBuf>,
    index_root: Option<PathBuf>,
    /// The walk can touch hundreds of thousands of files.
    index_rx: Option<Receiver<Vec<PathBuf>>>,
    /// Cached per `filter_gen`.
    results: Vec<PathBuf>,
    results_gen: Option<u64>,
    results_truncated: bool,
    /// Drained by the preview, to add to a scene.
    pending_adds: Vec<PathBuf>,
}

impl Explorer {
    pub fn selected_path(&self) -> Option<PathBuf> {
        self.selected.clone()
    }

    pub fn take_pending_adds(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.pending_adds)
    }

    /// Only when the user has not browsed anywhere, so a default never overrides a manual root.
    pub fn ensure_root(&mut self, dir: &Path) {
        if self.root.is_none() {
            set_root(self, dir.to_path_buf());
        }
    }

    /// Called when navigating away from the grid.
    pub fn cancel_grid(&mut self) {
        self.grid.stop();
    }

    pub fn invalidate_fs_cache(&mut self) {
        self.index_root = None;
        self.index_rx = None;
        self.results_gen = None;
        if let Some(root) = self.tree.as_mut() {
            root.children = None;
        }
    }
}

pub fn toolbar(
    explorer: &mut Explorer,
    root_dir: Option<&Path>,
    data_dir: Option<&Path>,
    mods_dir: Option<&Path>,
    show_tools: &mut bool,
    ui: &mut egui::Ui,
) {
    ui.horizontal(|ui| {
        if ui.button("Set root…").clicked()
            && let Some(d) = rfd::FileDialog::new().pick_folder()
        {
            set_root(explorer, d);
        }
        if let Some(r) = root_dir
            && r.is_dir()
            && ui.button("Game root").clicked()
        {
            set_root(explorer, r.to_path_buf());
        }
        if let Some(d) = data_dir
            && d.is_dir()
            && ui.button("Game data").clicked()
        {
            set_root(explorer, d.to_path_buf());
        }
        if let Some(m) = mods_dir
            && m.is_dir()
            && ui.button("Mods").clicked()
        {
            set_root(explorer, m.to_path_buf());
        }
        if let Some(r) = &explorer.root {
            ui.label(egui::RichText::new(r.display().to_string()).weak());
        }
    });
    ui.horizontal(|ui| {
        ui.label("Show:");
        egui::ComboBox::from_id_salt("kind_filter")
            .selected_text(filter_label(explorer.filter))
            .show_ui(ui, |ui| {
                for f in [
                    Filter::All,
                    Filter::Textures,
                    Filter::Audio,
                    Filter::Text,
                    Filter::Data,
                ] {
                    ui.selectable_value(&mut explorer.filter, f, filter_label(f));
                }
            });
        ui.label("Find:");
        ui.add(
            egui::TextEdit::singleline(&mut explorer.name_filter)
                .desired_width(170.0)
                .hint_text("fuzzy find…"),
        )
        .on_hover_text("Fuzzy-filters the tree and the folder grid by file name.");
        ui.toggle_value(show_tools, "Convert tools");
    });
}

/// Bounds memory and first-filter cost on a huge tree.
const INDEX_CAP: usize = 200_000;

pub fn tree(explorer: &mut Explorer, ui: &mut egui::Ui) {
    let key = (explorer.filter, explorer.name_filter.trim().to_owned());
    if explorer.filter_key != key {
        explorer.filter_key = key;
        explorer.filter_gen = explorer.filter_gen.wrapping_add(1);
    }
    if explorer.tree.is_none() {
        ui.weak("Set a root folder above to browse.");
        return;
    }
    let filtering = !explorer.filter_key.1.is_empty() || explorer.filter != Filter::All;
    if filtering {
        search_view(explorer, ui);
    } else {
        lazy_tree(explorer, ui);
    }
}

fn lazy_tree(explorer: &mut Explorer, ui: &mut egui::Ui) {
    let Explorer {
        tree,
        selected,
        pending_adds,
        ..
    } = explorer;
    let Some(root) = tree else { return };
    if root.children.is_none() {
        root.children = Some(read_children(&root.path));
    }
    if let Some(children) = root.children.as_mut() {
        for child in children.iter_mut() {
            draw_node(child, selected, pending_adds, ui);
        }
    }
}

fn draw_node(
    node: &mut Node,
    selected: &mut Option<PathBuf>,
    adds: &mut Vec<PathBuf>,
    ui: &mut egui::Ui,
) {
    if node.is_dir {
        let resp = egui::CollapsingHeader::new(node.name.clone())
            .id_salt(&node.path)
            .show(ui, |ui| {
                if node.children.is_none() {
                    node.children = Some(read_children(&node.path));
                }
                if let Some(children) = node.children.as_mut() {
                    for child in children.iter_mut() {
                        draw_node(child, selected, adds, ui);
                    }
                }
            });
        if resp.header_response.clicked() {
            *selected = Some(node.path.clone());
        }
    } else {
        let is_sel = selected.as_deref() == Some(node.path.as_path());
        if ui.selectable_label(is_sel, &node.name).clicked() {
            if ui.input(|i| i.modifiers.shift) {
                adds.push(node.path.clone());
            } else {
                *selected = Some(node.path.clone());
            }
        }
    }
}

/// A flat list rather than the tree with every folder force-open, which lagged badly on the
/// game-data root.
fn search_view(explorer: &mut Explorer, ui: &mut egui::Ui) {
    // The walk takes seconds on a game root, so a worker keeps typing responsive.
    if explorer.index_root.as_deref() != explorer.root.as_deref() {
        explorer.index_root = explorer.root.clone();
        explorer.file_index = Vec::new();
        explorer.results_gen = None;
        explorer.index_rx = None;
        if let Some(root) = explorer.root.clone() {
            let (tx, rx) = channel();
            explorer.index_rx = Some(rx);
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let _ = tx.send(build_index(&root, INDEX_CAP));
                ctx.request_repaint();
            });
        }
    }
    if let Some(rx) = &explorer.index_rx {
        match rx.try_recv() {
            Ok(index) => {
                explorer.results_truncated = index.len() >= INDEX_CAP;
                explorer.file_index = index;
                explorer.results_gen = None;
                explorer.index_rx = None;
            }
            Err(TryRecvError::Empty) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Indexing files…");
                });
                ui.ctx().request_repaint();
                return;
            }
            Err(TryRecvError::Disconnected) => explorer.index_rx = None,
        }
    }

    if explorer.results_gen != Some(explorer.filter_gen) {
        let matcher = SkimMatcherV2::default();
        let needle = explorer.filter_key.1.as_str();
        let filter = explorer.filter;
        let results: Vec<PathBuf> = if needle.is_empty() {
            // With no query there is no score to rank by, so keep the index's alphabetical order.
            explorer
                .file_index
                .iter()
                .filter(|p| filter_matches(filter, file_kind(p)))
                .cloned()
                .collect()
        } else {
            // A stable sort keeps equal scores alphabetical.
            let mut scored: Vec<(i64, &PathBuf)> = explorer
                .file_index
                .iter()
                .filter_map(|p| {
                    if !filter_matches(filter, file_kind(p)) {
                        return None;
                    }
                    let name = p
                        .file_name()
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default();
                    matcher
                        .fuzzy_match(name.as_ref(), needle)
                        .map(|score| (score, p))
                })
                .collect();
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            scored.into_iter().map(|(_, p)| p.clone()).collect()
        };
        explorer.results = results;
        explorer.results_gen = Some(explorer.filter_gen);
    }

    if explorer.results.is_empty() {
        ui.weak("No matching files.");
        return;
    }

    let root = explorer.root.clone().unwrap_or_default();
    let n = explorer.results.len();
    ui.horizontal(|ui| {
        ui.weak(format!("{n} match{}", if n == 1 { "" } else { "es" }));
        if explorer.results_truncated {
            ui.weak(format!("(index limited to {INDEX_CAP} files)"));
        }
    });

    // `show_rows` builds only the rows on screen, so match count does not matter.
    let row_height = ui.spacing().interact_size.y;
    let mut hit: Option<(usize, bool)> = None;
    egui::ScrollArea::vertical()
        .id_salt("tree_search")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, n, |ui, range| {
            for i in range {
                let path = &explorer.results[i];
                let rel = path.strip_prefix(&root).unwrap_or(path);
                let is_sel = explorer.selected.as_deref() == Some(path.as_path());
                if ui.selectable_label(is_sel, rel.to_string_lossy()).clicked() {
                    hit = Some((i, ui.input(|inp| inp.modifiers.shift)));
                }
            }
        });
    if let Some((i, shift)) = hit {
        if shift {
            explorer.pending_adds.push(explorer.results[i].clone());
        } else {
            explorer.selected = Some(explorer.results[i].clone());
        }
    }
}

/// Reads the type off the directory entry, to avoid an extra stat per file.
fn build_index(root: &Path, cap: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= cap {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let p = e.path();
            if is_dir {
                stack.push(p);
            } else if out.len() < cap {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn set_root(explorer: &mut Explorer, dir: PathBuf) {
    explorer.tree = Some(Node {
        name: dir.display().to_string(),
        path: dir.clone(),
        is_dir: true,
        children: None,
    });
    explorer.root = Some(dir);
    explorer.selected = None;
}

fn read_children(dir: &Path) -> Vec<Node> {
    let mut v: Vec<Node> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            let path = e.path();
            let is_dir = path.is_dir();
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            Node {
                path,
                name,
                is_dir,
                children: None,
            }
        })
        .collect();
    v.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
    });
    v
}

fn filter_matches(filter: Filter, kind: FileKind) -> bool {
    matches!(
        (filter, kind),
        (Filter::All, _)
            | (Filter::Textures, FileKind::Texture)
            | (Filter::Audio, FileKind::Audio)
            | (Filter::Text, FileKind::Text)
            | (Filter::Data, FileKind::Data)
    )
}

fn filter_label(f: Filter) -> &'static str {
    match f {
        Filter::All => "All files",
        Filter::Textures => "Textures",
        Filter::Audio => "Audio",
        Filter::Text => "Text",
        Filter::Data => "Databases",
    }
}

#[derive(Clone, Copy)]
enum ItemKind {
    Thumb,
    Audio,
    Text,
}

fn grid_item_kind(p: &Path) -> Option<ItemKind> {
    match ext_lower(p).as_deref() {
        Some("scd") => Some(ItemKind::Audio),
        Some("ztr" | "txt" | "xml") => Some(ItemKind::Text),
        Some("dds" | "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tga" | "webp") => {
            Some(ItemKind::Thumb)
        }
        Some("trb" | "xgr") if p.with_extension("imgb").is_file() => Some(ItemKind::Thumb),
        _ => None,
    }
}

enum GridMsg {
    Item {
        path: String,
        /// `None` for an icon tile.
        image: Option<egui::ColorImage>,
        badge: String,
    },
    Done,
}

struct GridEntry {
    path: String,
    thumb: Option<egui::TextureHandle>,
    badge: String,
}

#[derive(Default)]
struct DirGrid {
    dir: Option<PathBuf>,
    entries: Vec<GridEntry>,
    rx: Option<Receiver<GridMsg>>,
    scanning: bool,
    cancel: Arc<AtomicBool>,
    /// Cached, so the rescore and sort do not run every frame.
    view: Vec<usize>,
    view_key: Option<(String, usize)>,
}

impl DirGrid {
    /// Drops the thumbnails too, freeing their GPU textures.
    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.rx = None;
        self.scanning = false;
        self.dir = None;
        self.entries.clear();
        self.view.clear();
        self.view_key = None;
    }
}

/// Bounds a huge root, since the scan is recursive.
const GRID_CAP: usize = 600;

/// Clicking a thumbnail selects that file, which the parent then previews.
pub fn dir_grid(explorer: &mut Explorer, ui: &mut egui::Ui) {
    let Some(dir) = explorer.selected.clone() else {
        return;
    };
    if explorer.grid.dir.as_deref() != Some(dir.as_path()) {
        start_grid_scan(&mut explorer.grid, dir.clone(), ui.ctx());
    }
    drain_grid(&mut explorer.grid, ui.ctx());

    // The same search box that filters the tree narrows the grid.
    let needle = explorer.name_filter.trim().to_owned();
    let total = explorer.grid.entries.len();
    let key = (needle.clone(), total);
    if explorer.grid.view_key.as_ref() != Some(&key) {
        explorer.grid.view = if needle.is_empty() {
            (0..total).collect()
        } else {
            let matcher = SkimMatcherV2::default();
            let mut scored: Vec<(i64, usize)> = explorer
                .grid
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, e)| {
                    matcher
                        .fuzzy_match(grid_name(&e.path), &needle)
                        .map(|s| (s, i))
                })
                .collect();
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            scored.into_iter().map(|(_, i)| i).collect()
        };
        explorer.grid.view_key = Some(key);
    }
    let visible: Vec<&GridEntry> = explorer
        .grid
        .view
        .iter()
        .map(|&i| &explorer.grid.entries[i])
        .collect();

    ui.horizontal(|ui| {
        ui.strong(
            dir.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| dir.display().to_string()),
        );
        if explorer.grid.scanning {
            ui.spinner();
        }
        if needle.is_empty() {
            ui.label(format!("{total} item(s)"));
        } else {
            ui.label(format!("{} / {total} shown", visible.len()));
        }
        if total >= GRID_CAP {
            ui.weak(format!("(showing first {GRID_CAP})"));
        }
    });

    if visible.is_empty() {
        if total == 0 && !explorer.grid.scanning {
            ui.weak("No previewable files found under this folder.");
        } else if total > 0 {
            ui.weak("No items match the search.");
        }
        return;
    }

    let mut open = None;
    egui::ScrollArea::vertical()
        .id_salt("dir_grid")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Cells are exactly `CELL` wide, so wrapped rows stay aligned.
            ui.horizontal_wrapped(|ui| {
                for &entry in &visible {
                    if grid_cell(entry, ui) {
                        open = Some(entry.path.clone());
                    }
                }
            });
        });

    if let Some(p) = open {
        explorer.selected = Some(PathBuf::from(p));
    }
}

fn grid_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
}

const CELL: f32 = 96.0;

/// Exactly `CELL` wide, so wrapped rows stay aligned.
fn grid_cell(entry: &GridEntry, ui: &mut egui::Ui) -> bool {
    ui.allocate_ui_with_layout(
        egui::vec2(CELL, CELL + 20.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            let hover = format!("{}  ({})", entry.path, entry.badge);
            let clicked = match &entry.thumb {
                Some(handle) => {
                    let img = egui::Image::new(handle)
                        .fit_to_exact_size(egui::vec2(CELL, CELL))
                        .maintain_aspect_ratio(true);
                    ui.add(egui::ImageButton::new(img).frame(false))
                        .on_hover_text(hover)
                        .clicked()
                }
                None => {
                    let ext = Path::new(&entry.path)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("file");
                    ui.add_sized(
                        egui::vec2(CELL, CELL),
                        egui::Button::new(egui::RichText::new(ext).size(18.0)),
                    )
                    .on_hover_text(hover)
                    .clicked()
                }
            };
            let name = Path::new(&entry.path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            ui.add_sized(
                egui::vec2(CELL, 14.0),
                egui::Label::new(egui::RichText::new(name).small()).truncate(),
            );
            clicked
        },
    )
    .inner
}

fn start_grid_scan(grid: &mut DirGrid, dir: PathBuf, ctx: &egui::Context) {
    grid.cancel.store(true, Ordering::Relaxed);
    grid.cancel = Arc::new(AtomicBool::new(false));
    grid.dir = Some(dir.clone());
    grid.entries.clear();
    grid.view.clear();
    grid.view_key = None;
    grid.scanning = true;
    let (tx, rx) = channel();
    grid.rx = Some(rx);
    let ctx = ctx.clone();
    let cancel = grid.cancel.clone();
    std::thread::spawn(move || grid_worker(dir, tx, ctx, cancel));
}

fn grid_worker(dir: PathBuf, tx: Sender<GridMsg>, ctx: egui::Context, cancel: Arc<AtomicBool>) {
    use rayon::prelude::*;

    let mut items: Vec<(PathBuf, ItemKind)> = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        if cancel.load(Ordering::Relaxed) || items.len() >= GRID_CAP {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        let mut subdirs = Vec::new();
        for p in entries {
            if p.is_dir() {
                subdirs.push(p);
            } else if items.len() < GRID_CAP
                && let Some(kind) = grid_item_kind(&p)
            {
                items.push((p, kind));
            }
        }
        for sd in subdirs.into_iter().rev() {
            stack.push(sd);
        }
    }

    // Decoding runs on a capped pool, so a scan cannot peg every core.
    let decode = move || {
        items.par_iter().for_each_with(tx.clone(), |tx, (p, kind)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let msg = match kind {
                ItemKind::Audio => Some(GridMsg::Item {
                    path: p.display().to_string(),
                    image: None,
                    badge: "audio".into(),
                }),
                ItemKind::Text => Some(GridMsg::Item {
                    path: p.display().to_string(),
                    image: None,
                    badge: "text".into(),
                }),
                ItemKind::Thumb => {
                    super::texture::thumb_for(p).map(|(thumb, count)| GridMsg::Item {
                        path: p.display().to_string(),
                        image: Some(thumb),
                        badge: format!("{count} tex"),
                    })
                }
            };
            if let Some(msg) = msg {
                let _ = tx.send(msg);
                ctx.request_repaint();
            }
        });
        let _ = tx.send(GridMsg::Done);
        ctx.request_repaint();
    };
    match thumb_pool() {
        Some(pool) => pool.install(decode),
        None => decode(),
    }
}

/// Capped so a folder scan cannot saturate every core. `None` falls back to the global pool.
fn thumb_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        rayon::ThreadPoolBuilder::new()
            .num_threads((cores / 2).clamp(2, 8))
            .build()
            .ok()
    })
    .as_ref()
}

fn drain_grid(grid: &mut DirGrid, ctx: &egui::Context) {
    let mut incoming = Vec::new();
    let mut finished = false;
    if let Some(rx) = &grid.rx {
        for _ in 0..64 {
            match rx.try_recv() {
                Ok(GridMsg::Item { path, image, badge }) => incoming.push((path, image, badge)),
                Ok(GridMsg::Done) | Err(TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
    }
    for (path, image, badge) in incoming {
        let thumb = image.map(|ci| {
            ctx.load_texture(
                format!("grid_{}", grid.entries.len()),
                ci,
                egui::TextureOptions::LINEAR,
            )
        });
        grid.entries.push(GridEntry { path, thumb, badge });
    }
    if finished {
        grid.scanning = false;
        grid.rx = None;
    }
}
