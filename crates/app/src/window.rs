//! One editor window = one document.

use crate::glass::GlassPane;
use crate::icons;
use crate::render;
use crate::ribbon::Ribbon;
use adw::prelude::*;
use gtk::{cairo, gdk, gio, glib, graphene};
use hwpedit_core::{save, save_new, Format, OpenedFile, SaveError, SaveOptions, SaveOutcome};
use hwpedit_engine::{
    same_table, Align, CaretRect, CharFormat, Document, Hit, LineSpacingKind, ParaFormat, Pos,
    Range, Resize, SelRect,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

mod cells;
mod equation;
mod features;
mod lists;
mod page;
mod shapes;
mod view;
pub(crate) use cells::{BORDER_KINDS, BORDER_SIDES, BORDER_WIDTHS};
pub(crate) use lists::BULLETS;

const PAGE_GAP: i32 = 24;

type EditResult = Result<Option<Pos>, hwpedit_engine::EngineError>;
/// Order of the toolbar's alignment buttons (Hwp's: 양쪽, 왼쪽, 가운데, 오른쪽, 배분, 나눔).
const ALIGNS: [Align; 6] = [
    Align::Justify,
    Align::Left,
    Align::Center,
    Align::Right,
    Align::Distribute,
    Align::Split,
];
const TABLE_ACTIONS: [&str; 19] = [
    "table-row-equal",
    "table-copy",
    "table-select",
    "row-col-insert",
    "row-col-delete",
    "table-col-narrow",
    "table-col-wide",
    "table-row-short",
    "table-row-tall",
    "table-col-equal",
    "table-row-above",
    "table-row-below",
    "table-col-left",
    "table-col-right",
    "table-row-delete",
    "table-col-delete",
    "cell-merge",
    "cell-split",
    "table-delete",
];
const ZOOM_STEPS: [f64; 9] = [0.5, 0.67, 0.75, 0.9, 1.0, 1.25, 1.5, 2.0, 3.0];

struct FindBar {
    bar: gtk::SearchBar,
    query: gtk::SearchEntry,
    with: gtk::Entry,
    replace_row: gtk::Box,
    case: gtk::CheckButton,
    count: gtk::Label,
}

impl FindBar {
    fn new() -> FindBar {
        let query = gtk::SearchEntry::builder()
            .placeholder_text("찾을 내용")
            .width_chars(24)
            .build();
        let prev = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text("이전 (Shift+Enter)")
            .action_name("win.find-prev")
            .build();
        let next = gtk::Button::builder()
            .icon_name("go-down-symbolic")
            .tooltip_text("다음 (Enter)")
            .action_name("win.find-next")
            .build();
        let case = gtk::CheckButton::with_label("대소문자 구분");
        let count = gtk::Label::new(None);
        count.add_css_class("dim-label");
        count.set_width_chars(10);
        let find_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        find_row.append(&query);
        let nav = gtk::Box::builder().css_classes(["linked"]).build();
        nav.append(&prev);
        nav.append(&next);
        find_row.append(&nav);
        find_row.append(&count);
        find_row.append(&case);

        let with = gtk::Entry::builder()
            .placeholder_text("바꿀 내용")
            .width_chars(24)
            .build();
        let one = gtk::Button::builder()
            .label("바꾸기")
            .action_name("win.replace-one")
            .build();
        let all = gtk::Button::builder()
            .label("모두 바꾸기")
            .action_name("win.replace-all")
            .build();
        let replace_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        replace_row.append(&with);
        replace_row.append(&one);
        replace_row.append(&all);

        let col = gtk::Box::new(gtk::Orientation::Vertical, 6);
        col.append(&find_row);
        col.append(&replace_row);
        let bar = gtk::SearchBar::builder()
            .child(&col)
            .show_close_button(true)
            .build();
        bar.connect_entry(&query);
        FindBar {
            bar,
            query,
            with,
            replace_row,
            case,
            count,
        }
    }
}

/// A table cell border that can be dragged.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Border {
    page: u32,
    /// the cell whose right (or bottom) edge this is
    cell: Pos,
    /// true: a row border (drag up/down); false: a column border
    horizontal: bool,
    /// x (column border) or y (row border) in page pixels
    at: f64,
    /// another cell lies beyond: move the border; otherwise grow the table
    inner: bool,
}

/// A table's outline on a page, grabbed by its bottom-right corner to scale the table.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Corner {
    page: u32,
    cell: Pos,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

struct CachedPage {
    scale: f64,
    svg_hash: u64,
    /// The SVG drawn, to find what the next edit changed.
    svg: Rc<str>,
    /// The model changed since this was rendered; re-check the SVG before reuse.
    stale: bool,
    /// Lines were redrawn into this raster after an edit (see `render::rasterize_band`);
    /// the whole page is drawn again once typing pauses.
    patched: bool,
    surface: cairo::ImageSurface,
}

/// Copy a redrawn strip into the page raster. False if it does not fit (zoom changed).
fn paint_band(surface: &cairo::ImageSurface, b: render::Band) -> bool {
    if b.w as i32 != surface.width() || (b.y + b.h) as i32 > surface.height() {
        return false;
    }
    let Ok(strip) = render::surface_from_bgra(b.w, b.h, b.data) else {
        return false;
    };
    let Ok(cr) = cairo::Context::new(surface) else {
        return false;
    };
    cr.set_operator(cairo::Operator::Source);
    let _ = cr.set_source_surface(&strip, 0.0, b.y as f64);
    cr.rectangle(0.0, b.y as f64, b.w as f64, b.h as f64);
    let _ = cr.fill();
    drop(cr);
    surface.flush();
    true
}

/// HWPEDIT_IME_LOG=1: print input-method events to stderr, for reports of text landing
/// in the wrong place (they depend on the desktop's input method).
fn ime_log(what: std::fmt::Arguments) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("HWPEDIT_IME_LOG").is_some()) {
        eprintln!("ime {what}");
    }
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

struct Loaded {
    /// `None` for a new document that has never been saved.
    file: Option<OpenedFile>,
    doc: Document,
}

#[derive(Default)]
struct State {
    loaded: Option<Loaded>,
    caret: Option<Pos>,
    caret_rect: Option<CaretRect>,
    /// Other end of the selection, if any.
    anchor: Option<Pos>,
    sel_rects: Vec<SelRect>,
    /// Formatting chosen with no selection; applied to the next typed text.
    pending: Option<CharFormat>,
    find_hits: Vec<Range>,
    find_idx: Option<usize>,
    /// (sections, characters), refreshed after each model change.
    counts: (usize, usize),
    /// Table border under the pointer, and the one being dragged.
    border_hover: Option<Border>,
    border_drag: Option<(Border, f64)>,
    /// The table's bottom-right corner under the pointer, and the one being dragged (dx, dy).
    corner_hover: Option<Corner>,
    corner_drag: Option<(Corner, f64, f64)>,
    /// A table selected as an object (Esc in a cell, as in Hwp): any cell of it.
    table_selected: Option<Pos>,
    /// Text we put on the system clipboard when copying a table object.
    object_clip_text: Option<String>,
    preedit: String,
    zoom: f64,
    cache: HashMap<u32, CachedPage>,
    /// page -> (request id, svg hash, scale) of the raster being made in the background
    rendering: HashMap<u32, (u64, u64, f64)>,
    next_request: u64,
    /// Bumped on every model change: a raster begun before the latest change is shown
    /// but kept stale, so the next draw asks for a current one.
    model_gen: u64,
    /// User accepted the HWPX rewrite warning for this document.
    hwpx_rewrite_ok: bool,
    /// 모양 복사: the character and paragraph shape picked up with no selection.
    format_copy: Option<(hwpedit_engine::CharState, Option<hwpedit_engine::ParaState>)>,
    /// The caret sits right after an object just put in at this position (수식, 그림,
    /// 각주): typing goes after the object, not before it. Cleared when the caret moves.
    after_object: Option<Pos>,
}

pub struct Editor {
    pub win: adw::ApplicationWindow,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
    banner: adw::Banner,
    pages: gtk::Box,
    status: gtk::Label,
    zoom_scale: gtk::Scale,
    im: gtk::IMMulticontext,
    stack: gtk::Stack,
    scroller: gtk::ScrolledWindow,
    ribbon: Ribbon,
    find: FindBar,
    /// Set while pushing document state into the ribbon, so its signals don't edit.
    syncing: Cell<bool>,
    areas: RefCell<Vec<gtk::DrawingArea>>,
    /// Pending whole-page redraw after typing pauses (see `settle_later`).
    settle: Cell<Option<glib::SourceId>>,
    /// 보기 options that outlive a document: 회색조, 문단 부호, 투명 선.
    view: Cell<view::ViewOptions>,
    /// The panes floating over the top and bottom of the document view.
    chrome: (gtk::Widget, gtk::Widget),
    /// While a selection is being dragged: the page it started on and the pointer in
    /// the scroller's coordinates, and the timer that scrolls when the pointer is held
    /// past the top or bottom of the view.
    drag_pointer: Cell<Option<(u32, f64, f64)>>,
    autoscroll: Cell<Option<glib::SourceId>>,
    state: RefCell<State>,
}

impl Editor {
    pub fn new(app: &adw::Application) -> Rc<Editor> {
        let title = adw::WindowTitle::new("HWP 편집기", "");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));

        let menu = gio::Menu::new();
        let file_sec = gio::Menu::new();
        file_sec.append(Some("새 문서"), Some("app.new"));
        let recent = gio::Menu::new();
        file_sec.append_submenu(Some("최근 문서"), &recent);
        file_sec.append(Some("다른 이름으로 저장…"), Some("win.save-as"));
        menu.append_section(None, &file_sec);
        let out_sec = gio::Menu::new();
        out_sec.append(Some("PDF로 내보내기…"), Some("win.export-pdf"));
        out_sec.append(Some("인쇄…"), Some("win.print"));
        menu.append_section(None, &out_sec);
        let view_sec = gio::Menu::new();
        view_sec.append(Some("확대"), Some("win.zoom-in"));
        view_sec.append(Some("축소"), Some("win.zoom-out"));
        view_sec.append(Some("원래 크기"), Some("win.zoom-reset"));
        menu.append_section(None, &view_sec);
        let help_sec = gio::Menu::new();
        help_sec.append(Some("지원 범위와 제약"), Some("win.support"));
        help_sec.append(Some("정보"), Some("app.about"));
        menu.append_section(None, &help_sec);
        let menu_btn = gtk::MenuButton::builder()
            .icon_name(icons::name("menu"))
            .menu_model(&menu)
            .build();
        // refresh the recent list each time the menu opens
        menu_btn.connect_active_notify(move |b| {
            if b.is_active() {
                recent.remove_all();
                let items = recent_documents(10);
                if items.is_empty() {
                    recent.append(Some("(없음)"), Some("app.none"));
                }
                for p in items {
                    let label = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let item = gio::MenuItem::new(Some(&label), None);
                    item.set_action_and_target_value(
                        Some("app.open-recent"),
                        Some(&p.to_string_lossy().to_variant()),
                    );
                    recent.append_item(&item);
                }
            }
        });
        header.pack_end(&menu_btn);

        let banner = adw::Banner::new("");
        banner.set_button_label(Some("닫기"));
        banner.connect_button_clicked(|b| b.set_revealed(false));

        let pages = gtk::Box::new(gtk::Orientation::Vertical, PAGE_GAP);
        pages.set_halign(gtk::Align::Center);
        pages.set_margin_top(PAGE_GAP);
        pages.set_margin_bottom(PAGE_GAP);
        // Our own viewport: the one ScrolledWindow adds scrolls to whichever page takes
        // focus, which for a page taller than the window jumped the view away from the
        // caret (e.g. right after inserting a table). scroll_to_caret does the scrolling.
        let viewport = gtk::Viewport::builder()
            .child(&pages)
            .scroll_to_focus(false)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&viewport)
            .vexpand(true)
            .build();
        scroller.set_focusable(true);

        let placeholder = adw::StatusPage::builder()
            .icon_name("x-office-document-symbolic")
            .title("문서를 여세요")
            .description("HWP(.hwp) 또는 HWPX(.hwpx) 파일을 열거나 창으로 끌어 놓으세요.\n원본 파일은 저장 전까지 바뀌지 않으며, 저장할 때 백업됩니다.")
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&placeholder, Some("empty"));
        stack.add_named(&scroller, Some("doc"));

        // The canvas fills the window; glass panes float over it and blur what scrolls beneath.
        stack.add_css_class("canvas");
        stack.set_vexpand(true);
        let find = FindBar::new();
        let header_pane = GlassPane::new(&header, &stack, 18.0);
        let fonts = font_families(&scroller);
        let ribbon = Ribbon::new(&fonts, &stack);
        let find_pane = GlassPane::new(&find.bar, &stack, 16.0);
        find_pane.add_css_class("find");
        find_pane.set_halign(gtk::Align::End);
        let top = gtk::Box::new(gtk::Orientation::Vertical, 8);
        top.set_valign(gtk::Align::Start);
        for m in [
            &header_pane as &GlassPane,
            &ribbon.ribbon_pane,
            &ribbon.toolbar_pane,
        ] {
            top.append(m);
        }
        top.append(&banner);
        top.append(&find_pane);
        top.set_margin_top(10);
        top.set_margin_start(10);
        top.set_margin_end(10);

        let status = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        let zoom_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 10.0, 400.0, 5.0);
        zoom_scale.set_value(100.0);
        zoom_scale.set_draw_value(false);
        zoom_scale.set_width_request(160);
        zoom_scale.set_tooltip_text(Some("배율"));
        let status_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        status_box.add_css_class("status-bar");
        status_box.append(&status);
        status_box.append(&gtk::Image::from_icon_name(&icons::name("zoom-out")));
        status_box.append(&zoom_scale);
        status_box.append(&gtk::Image::from_icon_name(&icons::name("zoom-in")));
        // Hwp shows the zoom factor at the right end of the status bar
        status_box.append(&ribbon.zoom_label);
        let status_pane = GlassPane::new(&status_box, &stack, 16.0);
        status_pane.add_css_class("status");
        status_pane.set_valign(gtk::Align::End);
        status_pane.set_margin_start(10);
        status_pane.set_margin_end(10);
        status_pane.set_margin_bottom(10);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&stack));
        overlay.add_overlay(&top);
        overlay.add_overlay(&status_pane);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&overlay));

        // keep the first and last page clear of the floating panes
        {
            let (pages, top2, status_pane) = (pages.clone(), top.clone(), status_pane.clone());
            top.add_tick_callback(move |_, _| {
                let t = top2.height() + 10 + PAGE_GAP;
                let b = status_pane.height() + 10 + PAGE_GAP;
                if pages.margin_top() != t {
                    pages.set_margin_top(t);
                }
                if pages.margin_bottom() != b {
                    pages.set_margin_bottom(b);
                }
                glib::ControlFlow::Continue
            });
        }

        let win = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(1280)
            .default_height(860)
            .content(&toasts)
            .build();
        win.add_css_class("hwpedit");
        // GTK's prefers-color-scheme media query does not follow libadwaita here; mirror it as a class
        let sm = adw::StyleManager::default();
        let w = win.clone();
        let apply = move |sm: &adw::StyleManager| {
            if sm.is_dark() {
                w.add_css_class("dark");
            } else {
                w.remove_css_class("dark");
            }
        };
        apply(&sm);
        sm.connect_dark_notify(apply);

        let im = gtk::IMMulticontext::new();
        im.set_client_widget(Some(&scroller));

        let ed = Rc::new(Editor {
            win,
            title,
            toasts,
            banner,
            pages,
            status,
            zoom_scale,
            im,
            stack,
            scroller,
            ribbon,
            find,
            syncing: Cell::new(false),
            areas: RefCell::default(),
            settle: Cell::new(None),
            view: Cell::new(Default::default()),
            chrome: (top.clone().upcast(), status_pane.clone().upcast()),
            drag_pointer: Cell::new(None),
            autoscroll: Cell::new(None),
            state: RefCell::new(State {
                zoom: 1.0,
                ..Default::default()
            }),
        });
        ed.install_actions();
        ed.install_feature_actions();
        ed.install_list_actions();
        ed.install_cell_actions();
        ed.install_page_actions();
        ed.install_equation_actions();
        ed.install_shape_actions();
        ed.install_view_actions();
        ed.install_ribbon();
        ed.install_find();
        ed.install_input();
        ed.install_drop();
        ed.install_close_guard();
        ed.refresh_chrome();
        ed
    }

    pub fn has_document(&self) -> bool {
        self.state.borrow().loaded.is_some()
    }

    pub fn load(self: &Rc<Self>, path: &Path) {
        let result = OpenedFile::open(path)
            .map_err(|e| e.to_string())
            .and_then(|file| {
                Document::from_bytes(file.bytes(), file.format())
                    .map(|doc| Loaded {
                        file: Some(file),
                        doc,
                    })
                    .map_err(|e| e.to_string())
            });
        match result {
            Ok(loaded) => {
                let fmt = loaded.doc.format();
                {
                    let mut st = self.state.borrow_mut();
                    st.loaded = Some(loaded);
                    if let Some(l) = st.loaded.as_mut() {
                        self.view.get().apply(&mut l.doc);
                    }
                    st.caret = None;
                    st.caret_rect = None;
                    st.anchor = None;
                    st.sel_rects.clear();
                    st.pending = None;
                    st.find_hits.clear();
                    st.find_idx = None;
                    st.preedit.clear();
                    st.cache.clear();
                    st.hwpx_rewrite_ok = false;
                }
                let ext_mismatch = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_none_or(|e| !e.eq_ignore_ascii_case(fmt.extension()));
                if fmt == Format::Hwpx {
                    self.show_banner("HWPX 문서: 편집한 뒤 저장하면 엔진이 XML을 다시 씁니다(문단 ID 재번호 등). 저장할 때 원본은 백업됩니다.");
                } else if ext_mismatch {
                    self.show_banner(&format!(
                        "파일 확장자와 실제 형식이 다릅니다. 실제 형식은 {}이며, 이 형식 그대로 저장합니다.",
                        fmt.extension().to_uppercase()
                    ));
                } else {
                    self.banner.set_revealed(false);
                }
                self.rebuild_pages();
                self.stack.set_visible_child_name("doc");
                // like Hwp: an opened document has the caret at its start, so typing and
                // the formatting tools work right away
                self.set_caret(Some(Pos::Body {
                    section: 0,
                    para: 0,
                    offset: 0,
                }));
                self.focus_page();
            }
            Err(e) => self.alert(
                "문서를 열 수 없습니다",
                &format!("{}\n\n{e}", path.display()),
            ),
        }
        if self.has_document() {
            add_recent(path);
        }
        self.refresh_chrome();
    }

    /// Diagnostic: save the window with each ribbon tab in front as `dir/<tab>.png`
    /// (2x), then `dir/done`. Used to review the UI without clicking (HWPEDIT_UI_SHOTS).
    pub fn save_ui_shots(self: &Rc<Self>, dir: PathBuf) {
        let mut todo = self.ribbon.tab_ids();
        todo.reverse();
        let weak = Rc::downgrade(self);
        let mut pending: Option<String> = None;
        let mut tries = 0;
        glib::timeout_add_local(std::time::Duration::from_millis(700), move || {
            let Some(ed) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if let Some(id) = pending.take() {
                let snap = gtk::Snapshot::new();
                snap.scale(2.0, 2.0);
                // the window itself paints nothing through a WidgetPaintable; its content does
                let Some(body) = ed.win.content() else {
                    return glib::ControlFlow::Break;
                };
                let (w, h) = (body.width() as f64, body.height() as f64);
                gdk::prelude::PaintableExt::snapshot(
                    &gtk::WidgetPaintable::new(Some(&body)),
                    &snap,
                    w,
                    h,
                );
                if let Some(node) = snap.to_node() {
                    let r = gtk::gsk::CairoRenderer::new();
                    match r.realize_for_display(&WidgetExt::display(&ed.win)) {
                        Ok(()) => {
                            if let Err(e) = r
                                .render_texture(&node, None)
                                .save_to_png(dir.join(format!("{id}.png")))
                            {
                                eprintln!("ui shot {id}: {e}");
                            }
                            r.unrealize();
                        }
                        Err(e) => eprintln!("ui shot {id}: {e}"),
                    }
                } else if tries < 5 {
                    // not drawn yet (the tab switch has not been laid out); try again
                    tries += 1;
                    pending = Some(id);
                    return glib::ControlFlow::Continue;
                } else {
                    eprintln!("ui shot {id}: nothing drawn");
                }
                tries = 0;
            }
            match todo.pop() {
                Some(id) => {
                    ed.ribbon.select_tab(&id);
                    pending = Some(id);
                    glib::ControlFlow::Continue
                }
                None => {
                    let _ = std::fs::write(dir.join("done"), "");
                    glib::ControlFlow::Break
                }
            }
        });
    }

    /// Start a new, unsaved document from the blank template.
    pub fn load_new(self: &Rc<Self>) {
        match Document::new_blank() {
            Ok(doc) => {
                {
                    let mut st = self.state.borrow_mut();
                    *st = State {
                        zoom: st.zoom,
                        ..Default::default()
                    };
                    st.loaded = Some(Loaded { file: None, doc });
                    if let Some(l) = st.loaded.as_mut() {
                        self.view.get().apply(&mut l.doc);
                    }
                }
                self.banner.set_revealed(false);
                self.rebuild_pages();
                self.stack.set_visible_child_name("doc");
                self.set_caret(Some(Pos::Body {
                    section: 0,
                    para: 0,
                    offset: 0,
                }));
                self.focus_page();
            }
            Err(e) => self.alert("새 문서를 만들 수 없습니다", &e.to_string()),
        }
    }

    // ------------------------------------------------------------------ UI plumbing

    fn show_banner(&self, text: &str) {
        self.banner.set_title(text);
        self.banner.set_revealed(true);
    }

    fn toast(&self, text: &str) {
        self.toasts
            .add_toast(adw::Toast::builder().title(text).timeout(4).build());
    }

    fn alert(&self, heading: &str, body: &str) {
        let d = adw::AlertDialog::new(Some(heading), Some(body));
        d.add_response("ok", "확인");
        d.present(Some(&self.win));
    }

    fn set_enabled(&self, action: &str, on: bool) {
        if let Some(a) = self
            .win
            .lookup_action(action)
            .and_downcast::<gio::SimpleAction>()
        {
            a.set_enabled(on);
        }
    }

    fn refresh_chrome(&self) {
        {
            let st = self.state.borrow();
            let doc = st.loaded.as_ref().map(|l| &l.doc);
            self.set_enabled("undo", doc.is_some_and(Document::can_undo));
            self.set_enabled("redo", doc.is_some_and(Document::can_redo));
            self.set_enabled("save", doc.is_some());
            self.set_enabled("save-as", doc.is_some());
            self.set_enabled("export-pdf", doc.is_some());
            self.set_enabled("print", doc.is_some());
            let in_cell = matches!(st.caret, Some(Pos::Cell { .. }));
            let in_body = matches!(st.caret, Some(Pos::Body { .. }));
            self.set_enabled("page-break", in_body);
            self.set_enabled("char-shape", st.caret.is_some());
            self.set_enabled("para-shape", st.caret.is_some());
            self.set_enabled("insert-table", in_body);
            self.refresh_feature_actions(
                st.caret.is_some(),
                in_body,
                st.anchor.is_some_and(|a| Some(a) != st.caret),
            );
            self.refresh_list_actions(st.caret.is_some());
            self.refresh_page_actions(st.caret.is_some());
            self.set_enabled("equation", st.caret.is_some());
            self.refresh_shape_actions(in_body);
            self.refresh_cell_actions(matches!(st.caret, Some(Pos::Cell { .. })));
            for a in TABLE_ACTIONS {
                self.set_enabled(a, in_cell);
            }
            self.ribbon
                .zoom_label
                .set_label(&format!("{:.0}%", st.zoom * 100.0));
            self.ribbon.set_format_enabled(st.caret.is_some());
            self.ribbon.set_table_context(in_cell);
            if (self.zoom_scale.value() - st.zoom * 100.0).abs() > 0.5 {
                self.syncing.set(true);
                self.zoom_scale.set_value(st.zoom * 100.0);
                self.syncing.set(false);
            }
        }
        let st = self.state.borrow();
        let Some(l) = &st.loaded else {
            self.title.set_title("HWP 편집기");
            self.title.set_subtitle("");
            self.win.set_title(Some("HWP 편집기"));
            self.status.set_label("");
            return;
        };
        let name = l
            .file
            .as_ref()
            .and_then(|f| f.path().file_name())
            .map_or("새 문서".into(), |n| n.to_string_lossy().into_owned());
        let edited = l.doc.edited();
        let shown = if edited { format!("• {name}") } else { name };
        self.win.set_title(Some(&format!("{shown} — HWP 편집기")));
        self.title.set_title(&shown);
        self.title.set_subtitle(&format!(
            "{} · {}쪽{}",
            l.doc.format().extension().to_uppercase(),
            l.doc.page_count(),
            if edited { " · 수정됨" } else { "" }
        ));
        let path = l.file.as_ref().map_or("저장하지 않은 새 문서".into(), |f| {
            f.path().display().to_string()
        });
        // Hwp's status bar: 1/1쪽  1단  1칸  40글자  1/1 구역  삽입  변경 내용
        let (sections, chars) = st.counts;
        let mut parts = vec![match st.caret_rect {
            Some(r) => format!("{}/{}쪽", r.page + 1, l.doc.page_count()),
            None => format!("{}쪽", l.doc.page_count()),
        }];
        if let Some(p) = st.caret {
            let (sec, para) = match p {
                Pos::Body { section, para, .. } => (section, para),
                Pos::Cell {
                    section, cell_para, ..
                } => (section, cell_para),
            };
            parts.push(format!("{}문단", para + 1));
            parts.push(format!("{}칸", p.offset() + 1));
            parts.push(format!("{chars}글자"));
            parts.push(format!("{}/{} 구역", sec + 1, sections));
        } else {
            parts.push(format!("{chars}글자"));
        }
        parts.push("삽입".into());
        parts.push(if edited {
            "변경 내용 [수정됨]".into()
        } else {
            "변경 내용 없음".into()
        });
        parts.push(l.doc.format().extension().to_uppercase());
        parts.push(path);
        self.status.set_label(&parts.join("   "));
    }

    fn refresh_counts(&self) {
        let mut st = self.state.borrow_mut();
        st.counts = st.loaded.as_ref().map_or((0, 0), |l| l.doc.counts());
    }

    fn rebuild_pages(self: &Rc<Self>) {
        self.refresh_counts();
        while let Some(c) = self.pages.first_child() {
            self.pages.remove(&c);
        }
        let mut areas = Vec::new();
        let (count, sizes, zoom) = {
            let st = self.state.borrow();
            let Some(l) = &st.loaded else { return };
            let n = l.doc.page_count();
            let sizes: Vec<_> = (0..n).map(|p| l.doc.page_size(p).ok()).collect();
            (n, sizes, st.zoom)
        };
        for page in 0..count {
            let area = gtk::DrawingArea::new();
            if let Some(sz) = sizes[page as usize] {
                area.set_content_width((sz.width * zoom).round() as i32);
                area.set_content_height((sz.height * zoom).round() as i32);
            }
            area.add_css_class("page");
            let weak = Rc::downgrade(self);
            area.set_draw_func(move |a, cr, _, _| {
                if let Some(ed) = weak.upgrade() {
                    ed.draw_page(a, cr, page);
                }
            });
            let drag = gtk::GestureDrag::new();
            let weak = Rc::downgrade(self);
            drag.connect_drag_begin(move |g, x, y| {
                if let Some(ed) = weak.upgrade() {
                    // grabbing a table border starts a resize instead of placing the caret
                    let hover = ed.state.borrow().border_hover.filter(|b| b.page == page);
                    let corner = ed.state.borrow().corner_hover.filter(|c| c.page == page);
                    if let Some(c) = corner {
                        ed.state.borrow_mut().corner_drag = Some((c, 0.0, 0.0));
                    } else if let Some(b) = hover {
                        ed.state.borrow_mut().border_drag = Some((b, b.at));
                    } else {
                        let shift = g
                            .current_event_state()
                            .contains(gdk::ModifierType::SHIFT_MASK);
                        ed.on_press(page, x, y, shift);
                    }
                    if let Some(w) = g.widget() {
                        w.grab_focus();
                    }
                }
            });
            let weak = Rc::downgrade(self);
            drag.connect_drag_update(move |g, dx, dy| {
                let (Some(ed), Some((x, y))) = (weak.upgrade(), g.start_point()) else {
                    return;
                };
                if ed.state.borrow().corner_drag.is_some() {
                    ed.drag_corner(page, dx, dy, false);
                } else if ed.state.borrow().border_drag.is_some() {
                    ed.drag_border(page, dx, dy, false);
                } else {
                    ed.on_drag(page, x + dx, y + dy);
                    ed.track_drag_pointer(page, x + dx, y + dy);
                }
            });
            let weak = Rc::downgrade(self);
            drag.connect_drag_end(move |_, dx, dy| {
                if let Some(ed) = weak.upgrade() {
                    ed.stop_autoscroll();
                    if ed.state.borrow().corner_drag.is_some() {
                        ed.drag_corner(page, dx, dy, true);
                    } else if ed.state.borrow().border_drag.is_some() {
                        ed.drag_border(page, dx, dy, true);
                    }
                }
            });
            area.add_controller(drag);
            let menu_click = gtk::GestureClick::builder()
                .button(gdk::BUTTON_SECONDARY)
                .build();
            let weak = Rc::downgrade(self);
            menu_click.connect_pressed(move |g, _, x, y| {
                if let (Some(ed), Some(w)) = (weak.upgrade(), g.widget()) {
                    ed.context_menu(&w, page, x, y);
                }
            });
            area.add_controller(menu_click);
            // double-click on a 수식: edit it, as in Hwp
            let double = gtk::GestureClick::builder()
                .button(gdk::BUTTON_PRIMARY)
                .build();
            let weak = Rc::downgrade(self);
            double.connect_pressed(move |g, n, x, y| {
                if n == 2 {
                    if let Some(ed) = weak.upgrade() {
                        if ed.edit_equation_at(page, x, y) {
                            g.set_state(gtk::EventSequenceState::Claimed);
                        }
                    }
                }
            });
            area.add_controller(double);
            let motion = gtk::EventControllerMotion::new();
            let weak = Rc::downgrade(self);
            motion.connect_motion(move |c, x, y| {
                if let (Some(ed), Some(w)) = (weak.upgrade(), c.widget()) {
                    ed.hover_border(&w, page, x, y);
                }
            });
            area.add_controller(motion);
            area.set_focusable(true);
            self.pages.append(&area);
            areas.push(area);
        }
        *self.areas.borrow_mut() = areas;
    }

    fn resize_pages(&self) {
        let st = self.state.borrow();
        let Some(l) = &st.loaded else { return };
        for (p, a) in self.areas.borrow().iter().enumerate() {
            if let Ok(sz) = l.doc.page_size(p as u32) {
                a.set_content_width((sz.width * st.zoom).round() as i32);
                a.set_content_height((sz.height * st.zoom).round() as i32);
            }
            a.queue_draw();
        }
    }

    /// Paint the page from its cached raster. When the model changed, the page's SVG
    /// is regenerated here (cheap) and, if it differs, rasterized on a worker thread
    /// (~100 ms for a dense page) while the previous raster stays on screen, so typing
    /// never waits for rendering.
    fn draw_page(self: &Rc<Self>, area: &gtk::DrawingArea, cr: &cairo::Context, page: u32) {
        let device = self.state.borrow().zoom * area.scale_factor() as f64;
        self.ensure_raster(page, device);
        let st = self.state.borrow_mut();
        let zoom = st.zoom;
        match st.cache.get(&page) {
            Some(c) => {
                let _ = cr.save();
                // an older raster may be at another zoom until the new one arrives
                let k = 1.0 / area.scale_factor() as f64 * (device / c.scale);
                cr.scale(k, k);
                let _ = cr.set_source_surface(&c.surface, 0.0, 0.0);
                let _ = cr.paint();
                let _ = cr.restore();
            }
            None => {
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.paint();
            }
        }

        // a table selected as an object: outline and handles, as in Hwp
        if let Some(t) = st.table_selected {
            if let Some(l) = &st.loaded {
                let b = l.doc.cell_boxes(t, page);
                if !b.is_empty() {
                    let x0 = b.iter().map(|b| b.x).fold(f64::MAX, f64::min) * zoom;
                    let y0 = b.iter().map(|b| b.y).fold(f64::MAX, f64::min) * zoom;
                    let x1 = b.iter().map(|b| b.x + b.w).fold(f64::MIN, f64::max) * zoom;
                    let y1 = b.iter().map(|b| b.y + b.h).fold(f64::MIN, f64::max) * zoom;
                    cr.set_source_rgba(0.21, 0.52, 0.89, 0.12);
                    cr.rectangle(x0, y0, x1 - x0, y1 - y0);
                    let _ = cr.fill();
                    cr.set_source_rgba(0.21, 0.52, 0.89, 1.0);
                    cr.set_line_width(1.0);
                    cr.rectangle(x0 + 0.5, y0 + 0.5, x1 - x0, y1 - y0);
                    let _ = cr.stroke();
                    let (xm, ym) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                    for (hx, hy) in [
                        (x0, y0),
                        (xm, y0),
                        (x1, y0),
                        (x0, ym),
                        (x1, ym),
                        (x0, y1),
                        (xm, y1),
                        (x1, y1),
                    ] {
                        cr.rectangle(hx - 3.5, hy - 3.5, 7.0, 7.0);
                        cr.set_source_rgb(1.0, 1.0, 1.0);
                        let _ = cr.fill_preserve();
                        cr.set_source_rgba(0.21, 0.52, 0.89, 1.0);
                        let _ = cr.stroke();
                    }
                }
            }
        }

        // the table being scaled from its corner: its new outline
        if let Some((c, dx, dy)) = st.corner_drag.filter(|(c, _, _)| c.page == page) {
            cr.set_source_rgba(0.21, 0.52, 0.89, 0.9);
            cr.set_line_width(1.5);
            cr.set_dash(&[4.0, 3.0], 0.0);
            let (w, h) = (
                ((c.x1 - c.x0) * zoom + dx).max(8.0),
                ((c.y1 - c.y0) * zoom + dy).max(8.0),
            );
            cr.rectangle(c.x0 * zoom, c.y0 * zoom, w, h);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        }

        // the border being dragged: a guide line where it will land
        if let Some((b, at)) = st.border_drag.filter(|(b, _)| b.page == page) {
            cr.set_source_rgba(0.21, 0.52, 0.89, 0.9);
            cr.set_line_width(1.5);
            cr.set_dash(&[4.0, 3.0], 0.0);
            if b.horizontal {
                cr.move_to(0.0, at * zoom);
                cr.line_to(area.width() as f64, at * zoom);
            } else {
                cr.move_to(at * zoom, 0.0);
                cr.line_to(at * zoom, area.height() as f64);
            }
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        }

        // Hwp-style corner marks around the text area
        if let Some(sz) = st.loaded.as_ref().and_then(|l| l.doc.page_size(page).ok()) {
            let (ml, mt, mr, mb) = sz.margins;
            let (x0, y0, x1, y1) = (
                ml * zoom,
                mt * zoom,
                (sz.width - mr) * zoom,
                (sz.height - mb) * zoom,
            );
            let len = 18.0 * zoom;
            cr.set_source_rgba(0.55, 0.58, 0.63, 0.9);
            cr.set_line_width(1.0);
            for (x, y, dx, dy) in [
                (x0, y0, -1.0, -1.0),
                (x1, y0, 1.0, -1.0),
                (x0, y1, -1.0, 1.0),
                (x1, y1, 1.0, 1.0),
            ] {
                let (x, y) = (x.round() + 0.5, y.round() + 0.5);
                cr.move_to(x + dx * len, y);
                cr.line_to(x, y);
                cr.line_to(x, y + dy * len);
            }
            let _ = cr.stroke();
        }

        cr.set_source_rgba(0.21, 0.52, 0.89, 0.30);
        for r in st.sel_rects.iter().filter(|r| r.page == page) {
            cr.rectangle(r.x * zoom, r.y * zoom, r.w * zoom, r.h * zoom);
        }
        let _ = cr.fill();

        if let Some(r) = st.caret_rect.filter(|r| r.page == page) {
            let (x, y, h) = (r.x * zoom, r.y * zoom, r.height.max(8.0) * zoom);
            if !st.preedit.is_empty() {
                // the syllable being composed, in the face and size it will be typed in
                let mut cs = st
                    .loaded
                    .as_ref()
                    .zip(st.caret)
                    .and_then(|(l, p)| l.doc.char_state(p).ok());
                if let (Some(c), Some(f)) = (cs.as_mut(), &st.pending) {
                    overlay(c, f);
                }
                let (font, size, bold) = cs
                    .map_or(("Malgun Gothic".to_owned(), h * 0.75, false), |c| {
                        (c.font, c.size_pt * 96.0 / 72.0 * zoom, c.bold)
                    });
                cr.select_font_face(
                    &font,
                    cairo::FontSlant::Normal,
                    if bold {
                        cairo::FontWeight::Bold
                    } else {
                        cairo::FontWeight::Normal
                    },
                );
                cr.set_font_size(size);
                let w = cr
                    .text_extents(&st.preedit)
                    .map(|e| e.x_advance())
                    .unwrap_or(h);
                cr.set_source_rgba(1.0, 0.97, 0.75, 1.0);
                cr.rectangle(x, y, w, h);
                let _ = cr.fill();
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.move_to(x, y + h * 0.8);
                let _ = cr.show_text(&st.preedit);
                cr.rectangle(x, y + h - 1.5, w, 1.5);
                let _ = cr.fill();
            } else {
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.rectangle(x.round() - 0.5, y, 1.5, h);
                let _ = cr.fill();
            }
        }
    }

    // ------------------------------------------------------------------ editing

    /// Find a table border within a few pixels of the pointer and show a resize cursor.
    fn hover_border(&self, w: &gtk::Widget, page: u32, x: f64, y: f64) {
        if self.state.borrow().border_drag.is_some() || self.state.borrow().corner_drag.is_some() {
            return;
        }
        let zoom = self.state.borrow().zoom;
        let corner = (|| {
            let st = self.state.borrow();
            let l = st.loaded.as_ref()?;
            let (px, py, tol) = (x / zoom, y / zoom, 6.0 / zoom);
            let Ok(Hit::Editable(p @ Pos::Cell { .. })) = l.doc.hit_test(page, px - tol, py - tol)
            else {
                return None;
            };
            let b = l.doc.cell_boxes(p, page);
            let x0 = b.iter().map(|b| b.x).fold(f64::MAX, f64::min);
            let y0 = b.iter().map(|b| b.y).fold(f64::MAX, f64::min);
            let x1 = b.iter().map(|b| b.x + b.w).fold(f64::MIN, f64::max);
            let y1 = b.iter().map(|b| b.y + b.h).fold(f64::MIN, f64::max);
            ((px - x1).abs() <= tol && (py - y1).abs() <= tol).then_some(Corner {
                page,
                cell: p,
                x0,
                y0,
                x1,
                y1,
            })
        })();
        self.state.borrow_mut().corner_hover = corner;
        if corner.is_some() {
            self.state.borrow_mut().border_hover = None;
            w.set_cursor_from_name(Some("nwse-resize"));
            return;
        }
        let (px, py) = (x / zoom, y / zoom);
        let tol = 4.0 / zoom;
        let found = (|| {
            let st = self.state.borrow();
            let l = st.loaded.as_ref()?;
            let Ok(Hit::Editable(p @ Pos::Cell { .. })) = l.doc.hit_test(page, px, py) else {
                return None;
            };
            let boxes = l.doc.cell_boxes(p, page);
            let Pos::Cell {
                section,
                parent_para,
                control,
                ..
            } = p
            else {
                return None;
            };
            let cell_pos = |i: usize| Pos::Cell {
                section,
                parent_para,
                control,
                cell: i,
                cell_para: 0,
                offset: 0,
            };
            for b in &boxes {
                let (right, bottom) = (b.x + b.w, b.y + b.h);
                if (px - right).abs() <= tol && py >= b.y && py <= bottom {
                    let inner = boxes
                        .iter()
                        .any(|o| (o.x - right).abs() < 1.0 && o.y < bottom && o.y + o.h > b.y);
                    return Some(Border {
                        page,
                        cell: cell_pos(b.cell),
                        horizontal: false,
                        at: right,
                        inner,
                    });
                }
                if (py - bottom).abs() <= tol && px >= b.x && px <= right {
                    let inner = boxes
                        .iter()
                        .any(|o| (o.y - bottom).abs() < 1.0 && o.x < right && o.x + o.w > b.x);
                    return Some(Border {
                        page,
                        cell: cell_pos(b.cell),
                        horizontal: true,
                        at: bottom,
                        inner,
                    });
                }
            }
            None
        })();
        let cursor = found.map(|b| {
            if b.horizontal {
                "row-resize"
            } else {
                "col-resize"
            }
        });
        w.set_cursor_from_name(cursor);
        self.state.borrow_mut().border_hover = found;
    }

    /// Move the guide while dragging; on release apply the resize as one undo step.
    fn drag_border(self: &Rc<Self>, page: u32, dx: f64, dy: f64, done: bool) {
        let (b, zoom) = {
            let st = self.state.borrow();
            let Some((b, _)) = st.border_drag else { return };
            (b, st.zoom)
        };
        let moved = if b.horizontal { dy } else { dx } / zoom;
        let step_px = hwpedit_engine::RESIZE_STEP_HWPUNIT / hwpedit_engine::HWPUNIT_PER_PX;
        let steps = (moved / step_px).round() as i32;
        self.state.borrow_mut().border_drag = Some((b, b.at + steps as f64 * step_px));
        if let Some(a) = self.areas.borrow().get(page as usize) {
            a.queue_draw();
        }
        if !done {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            st.border_drag = None;
            st.border_hover = None;
        }
        if steps != 0 {
            let mode = if b.inner {
                Resize::Border
            } else {
                Resize::Table
            };
            let cell = b.cell;
            self.state.borrow_mut().caret = Some(cell);
            self.edit(move |d, _, _| d.resize_table(cell, b.horizontal, steps, mode).map(Some));
        }
    }

    /// Scale the whole table when its corner is released.
    fn drag_corner(self: &Rc<Self>, page: u32, dx: f64, dy: f64, done: bool) {
        let (c, zoom) = {
            let st = self.state.borrow();
            let Some((c, _, _)) = st.corner_drag else {
                return;
            };
            (c, st.zoom)
        };
        self.state.borrow_mut().corner_drag = Some((c, dx, dy));
        if let Some(a) = self.areas.borrow().get(page as usize) {
            a.queue_draw();
        }
        if !done {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            st.corner_drag = None;
            st.corner_hover = None;
        }
        let (w, h) = (c.x1 - c.x0, c.y1 - c.y0);
        let fx = ((w + dx / zoom) / w).max(0.2);
        let fy = ((h + dy / zoom) / h).max(0.2);
        if (fx - 1.0).abs() < 0.01 && (fy - 1.0).abs() < 0.01 {
            return;
        }
        let rows_ok = Rc::new(Cell::new(true));
        let flag = rows_ok.clone();
        let cell = c.cell;
        self.state.borrow_mut().caret = Some(cell);
        self.edit(move |d, _, _| {
            d.scale_table(cell, fx, fy).map(|(p, ok)| {
                flag.set(ok);
                Some(p)
            })
        });
        if !rows_ok.get() {
            self.toast(
                "합쳐진 셀이 있는 표라 너비만 바꿨습니다 (줄 높이 조절은 아직 지원하지 않습니다)",
            );
        }
    }

    fn hit(&self, page: u32, x: f64, y: f64) -> Option<Result<Hit, hwpedit_engine::EngineError>> {
        let st = self.state.borrow();
        let l = st.loaded.as_ref()?;
        Some(l.doc.hit_test(page, x / st.zoom, y / st.zoom))
    }

    fn on_press(&self, page: u32, x: f64, y: f64, extend: bool) {
        let at_object = {
            let st = self.state.borrow();
            st.loaded
                .as_ref()
                .and_then(|l| {
                    l.doc
                        .hit_test_at_object(page, x / st.zoom, y / st.zoom)
                        .ok()
                })
                .is_some_and(|h| h.1)
        };
        match self.hit(page, x, y) {
            Some(Ok(Hit::Editable(pos))) => {
                self.place(Some(pos), extend);
                // clicked just after a 수식/각주: the caret and typing go after it
                if at_object && !extend {
                    self.state.borrow_mut().after_object = Some(pos);
                    self.set_caret_keep_anchor(Some(pos));
                }
            }
            Some(Ok(Hit::ReadOnly(why))) => {
                self.set_caret(None);
                self.toast(&why);
            }
            Some(Ok(Hit::Nothing)) => self.set_caret(None),
            Some(Err(e)) => self.toast(&e.to_string()),
            None => {}
        }
    }

    /// Extend the selection to the pointer. (`x`, `y`) are in the coordinates of the page
    /// the drag started on; once the pointer is over another page (or the gap between
    /// pages), the point is taken on that page, so a selection can run across pages.
    fn on_drag(&self, page: u32, x: f64, y: f64) {
        if self.state.borrow().caret.is_none() {
            return;
        }
        let (page, x, y) = self.page_under(page, x, y);
        if let Some(Ok(Hit::Editable(pos))) = self.hit(page, x, y) {
            self.place(Some(pos), true);
        }
    }

    /// Remember where the dragging pointer is, in the scroller's coordinates, and start
    /// scrolling when it is past the top or bottom of the visible page area.
    fn track_drag_pointer(self: &Rc<Self>, page: u32, x: f64, y: f64) {
        let Some(area) = self.areas.borrow().get(page as usize).cloned() else {
            return;
        };
        let Some(p) = area.compute_point(&self.scroller, &graphene::Point::new(x as f32, y as f32))
        else {
            return;
        };
        self.drag_pointer
            .set(Some((page, p.x() as f64, p.y() as f64)));
        if self.autoscroll_step_px() != 0.0 {
            self.start_autoscroll();
        }
    }

    /// Top and bottom of the document view not covered by the floating panes, in the
    /// scroller's coordinates.
    fn visible_band(&self) -> (f64, f64) {
        let h = self.scroller.height() as f64;
        let top = self
            .chrome
            .0
            .compute_bounds(&self.scroller)
            .map_or(0.0, |b| (b.y() + b.height()) as f64);
        let bottom = self
            .chrome
            .1
            .compute_bounds(&self.scroller)
            .map_or(h, |b| b.y() as f64);
        (top.clamp(0.0, h), bottom.clamp(0.0, h))
    }

    /// How far to scroll per tick for the current drag pointer: negative up, positive
    /// down, 0 inside the view. Faster the further past the edge, as in Hwp.
    fn autoscroll_step_px(&self) -> f64 {
        let Some((_, _, y)) = self.drag_pointer.get() else {
            return 0.0;
        };
        const EDGE: f64 = 24.0;
        let (top, bottom) = self.visible_band();
        let speed = |past: f64| (past * 0.5).clamp(4.0, 60.0);
        if y < top + EDGE {
            -speed(top + EDGE - y)
        } else if y > bottom - EDGE {
            speed(y - (bottom - EDGE))
        } else {
            0.0
        }
    }

    fn start_autoscroll(self: &Rc<Self>) {
        let running = self.autoscroll.take();
        if running.is_some() {
            self.autoscroll.set(running);
            return;
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local(std::time::Duration::from_millis(30), move || {
            let Some(ed) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let step = ed.autoscroll_step_px();
            let Some((page, sx, sy)) = ed.drag_pointer.get().filter(|_| step != 0.0) else {
                ed.autoscroll.set(None);
                return glib::ControlFlow::Break;
            };
            let adj = ed.scroller.vadjustment();
            let before = adj.value();
            adj.set_value((before + step).clamp(adj.lower(), adj.upper() - adj.page_size()));
            if adj.value() == before {
                return glib::ControlFlow::Continue; // at the top or the end
            }
            // the pointer has not moved on screen, but the pages have under it
            let area = ed.areas.borrow().get(page as usize).cloned();
            if let Some(p) = area.and_then(|a| {
                ed.scroller
                    .compute_point(&a, &graphene::Point::new(sx as f32, sy as f32))
            }) {
                ed.on_drag(page, p.x() as f64, p.y() as f64);
            }
            glib::ControlFlow::Continue
        });
        self.autoscroll.set(Some(id));
    }

    fn stop_autoscroll(&self) {
        self.drag_pointer.set(None);
        if let Some(id) = self.autoscroll.take() {
            id.remove();
        }
    }

    /// The page under a point given in page `from`'s coordinates, and the point in that
    /// page's coordinates. Between pages: the nearer page, with y clamped onto it.
    fn page_under(&self, from: u32, x: f64, y: f64) -> (u32, f64, f64) {
        let areas = self.areas.borrow();
        let Some(origin) = areas.get(from as usize) else {
            return (from, x, y);
        };
        let mut best: Option<(f64, u32, f64, f64)> = None;
        for (i, a) in areas.iter().enumerate() {
            let Some(p) = origin.compute_point(a, &gtk::graphene::Point::new(x as f32, y as f32))
            else {
                continue;
            };
            let (px, py, h) = (p.x() as f64, p.y() as f64, a.height() as f64);
            let dist = if py < 0.0 {
                -py
            } else if py > h {
                py - h
            } else {
                0.0
            };
            if best.is_none_or(|b| dist < b.0) {
                best = Some((dist, i as u32, px, py.clamp(0.0, (h - 1.0).max(0.0))));
            }
        }
        best.map_or((from, x, y), |(_, p, px, py)| (p, px, py))
    }

    /// Non-empty selection, if any.
    fn selection(&self) -> Option<Range> {
        let st = self.state.borrow();
        Range::new(st.anchor?, st.caret?).filter(|r| !r.is_empty())
    }

    /// Move the caret by user navigation (ends a typing run). `extend` keeps or starts a selection.
    fn place(&self, pos: Option<Pos>, extend: bool) {
        {
            let mut st = self.state.borrow_mut();
            st.anchor = if extend { st.anchor.or(st.caret) } else { None };
            st.pending = None;
            st.after_object = None;
            st.table_selected = None;
            if let Some(l) = st.loaded.as_mut() {
                l.doc.break_coalescing();
            }
        }
        self.set_caret_keep_anchor(pos);
    }

    /// Set the caret and drop any selection.
    fn set_caret(&self, pos: Option<Pos>) {
        self.state.borrow_mut().anchor = None;
        self.set_caret_keep_anchor(pos);
    }

    fn set_caret_keep_anchor(&self, pos: Option<Pos>) {
        let old_pages: Vec<u32> = {
            let st = self.state.borrow();
            st.caret_rect
                .iter()
                .map(|r| r.page)
                .chain(st.sel_rects.iter().map(|r| r.page))
                .collect()
        };
        let (rect, new_pages) = {
            let mut st = self.state.borrow_mut();
            st.caret = pos;
            st.caret_rect = match (&st.loaded, pos) {
                (Some(l), Some(p)) if st.after_object == Some(p) => {
                    l.doc.caret_rect_after_object(p).ok()
                }
                (Some(l), Some(p)) => l.doc.caret_rect(p).ok(),
                _ => None,
            };
            // a selection must stay inside one body section or one cell
            let range = match (st.anchor, pos) {
                (Some(a), Some(p)) => Range::new(a, p),
                _ => None,
            };
            // ...except a block of cells in one table, kept for "merge cells"
            let cell_block = match (st.anchor, pos) {
                (Some(a), Some(p)) if range.is_none() && same_table(a, p) => Some((a, p)),
                _ => None,
            };
            if range.is_none() && cell_block.is_none() {
                st.anchor = None;
            }
            st.sel_rects = match (&st.loaded, range, cell_block) {
                (Some(l), Some(r), _) => l.doc.selection_rects(r),
                (Some(l), None, Some((a, p))) => {
                    cell_block_rect(&l.doc, a, p).into_iter().collect()
                }
                _ => Vec::new(),
            };
            let pages: Vec<u32> = st
                .caret_rect
                .iter()
                .map(|r| r.page)
                .chain(st.sel_rects.iter().map(|r| r.page))
                .collect();
            (st.caret_rect, pages)
        };
        let areas = self.areas.borrow();
        for p in old_pages.into_iter().chain(new_pages) {
            if let Some(a) = areas.get(p as usize) {
                a.queue_draw();
            }
        }
        if let Some(r) = rect {
            if let Some(a) = areas.get(r.page as usize) {
                let zoom = self.state.borrow().zoom;
                if let Some(pt) = a.compute_point(
                    &self.scroller,
                    &graphene::Point::new((r.x * zoom) as f32, (r.y * zoom) as f32),
                ) {
                    self.im.set_cursor_location(&gdk::Rectangle::new(
                        pt.x() as i32,
                        pt.y() as i32,
                        2,
                        (r.height * zoom) as i32,
                    ));
                }
            }
        }
        drop(areas);
        self.sync_ribbon();
        self.refresh_chrome();
    }

    /// Push the caret's formatting into the ribbon without triggering edits.
    fn sync_ribbon(&self) {
        let (cs, align, styles, style) = {
            let st = self.state.borrow();
            let (Some(l), Some(p)) = (&st.loaded, st.caret) else {
                return;
            };
            // with a selection, show the formatting at its start
            let at = Range::new(st.anchor.unwrap_or(p), p).map_or(p, |r| {
                if r.is_empty() {
                    p
                } else {
                    r.start.with_offset(r.start.offset() + 1)
                }
            });
            let mut cs = l.doc.char_state(at).ok();
            if let (Some(c), Some(f)) = (cs.as_mut(), &st.pending) {
                overlay(c, f);
            }
            (cs, l.doc.para_state(p), l.doc.styles(), l.doc.style_at(p))
        };
        self.syncing.set(true);
        let rb = &self.ribbon;
        rb.show_styles(
            &styles.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            style,
        );
        if let Some(c) = cs {
            rb.bold.set_active(c.bold);
            rb.italic.set_active(c.italic);
            rb.underline.set_active(c.underline);
            rb.strike.set_active(c.strike);
            rb.size.set_value(c.size_pt);
            rb.show_font(&c.font);
            if let Ok(rgba) = gdk::RGBA::parse(c.color.as_str()) {
                rb.color.set_rgba(&rgba);
            }
        }
        if let Some(ps) = align {
            match ALIGNS.iter().position(|x| *x == ps.align) {
                Some(i) => rb.align[i].set_active(true),
                None => rb.align.iter().for_each(|b| b.set_active(false)),
            }
            rb.line_spacing.set_sensitive(ps.line_spacing_pct.is_some());
            if let Some(ls) = ps.line_spacing_pct {
                rb.line_spacing.set_value(ls);
            }
        }
        self.syncing.set(false);
    }

    /// Make sure a raster for `page` at `device` scale is current or on its way.
    fn ensure_raster(self: &Rc<Self>, page: u32, device: f64) {
        let job = {
            let mut st = self.state.borrow_mut();
            let cached = st.cache.get(&page);
            let fresh = cached.is_some_and(|c| !c.stale && (c.scale - device).abs() < 1e-6);
            if fresh {
                return;
            }
            let Some(l) = &st.loaded else { return };
            let Ok(svg) = l.doc.render_page_svg(page) else {
                return;
            };
            let h = hash_str(&svg);
            let svg: Rc<str> = svg.into();
            if let Some(c) = st.cache.get_mut(&page) {
                if (c.scale - device).abs() < 1e-6 {
                    if c.svg_hash == h {
                        c.stale = false; // the edit did not touch this page
                        return;
                    }
                    // typing or deleting on a line: redraw just those lines now, so the
                    // text shows at once instead of after the whole page (~100 ms)
                    if let Some(mut b) = render::rasterize_band(&c.svg, &svg, device) {
                        if self.view.get().gray {
                            render::grayscale(&mut b.data);
                        }
                        if paint_band(&c.surface, b) {
                            c.svg_hash = h;
                            c.svg = svg;
                            c.stale = false;
                            c.patched = true;
                            st.rendering.remove(&page); // older than this
                            drop(st);
                            self.settle_later();
                            return;
                        }
                    }
                }
            }
            if st
                .rendering
                .get(&page)
                .is_some_and(|&(_, ph, ps)| ph == h && (ps - device).abs() < 1e-6)
            {
                return; // already being rendered
            }
            (svg, h)
        };
        self.render_whole_page(page, device, job.0, job.1);
    }

    /// Rasterize the whole page on a worker thread; the old raster stays on screen meanwhile.
    fn render_whole_page(self: &Rc<Self>, page: u32, device: f64, svg: Rc<str>, h: u64) {
        let gen = {
            let mut st = self.state.borrow_mut();
            st.next_request += 1;
            let id = st.next_request;
            st.rendering.insert(page, (id, h, device));
            (id, st.model_gen)
        };
        let (id, gen) = gen;
        let weak = Rc::downgrade(self);
        let text = svg.to_string();
        let gray = self.view.get().gray;
        let job = gio::spawn_blocking(move || {
            render::rasterize_bgra(&text, device).map(|(w, h, mut data)| {
                if gray {
                    render::grayscale(&mut data);
                }
                (w, h, data)
            })
        });
        glib::spawn_future_local(async move {
            let Ok(result) = job.await else { return };
            let Some(ed) = weak.upgrade() else { return };
            {
                let mut st = ed.state.borrow_mut();
                // a newer request for this page supersedes this one
                if st.rendering.get(&page).map(|p| p.0) != Some(id) {
                    return;
                }
                st.rendering.remove(&page);
                match result.and_then(|(w, hgt, data)| render::surface_from_bgra(w, hgt, data)) {
                    Ok(surface) => {
                        // the document may have changed while this was drawn (fast typing on a
                        // slow page): show it, but as stale, or the newer text never appears
                        let stale = gen != st.model_gen;
                        st.cache.insert(
                            page,
                            CachedPage {
                                scale: device,
                                svg_hash: h,
                                svg,
                                stale,
                                patched: false,
                                surface,
                            },
                        );
                    }
                    Err(e) => {
                        drop(st);
                        ed.toast(&format!("{}쪽을 그릴 수 없습니다: {e}", page + 1));
                        return;
                    }
                }
            }
            let area = ed.areas.borrow().get(page as usize).cloned();
            if let Some(a) = area {
                a.queue_draw();
            }
        });
    }

    /// Once typing has paused, draw patched pages whole again: a strip drawn on its own
    /// can differ from the full page by a pixel's shade at its edges.
    fn settle_later(self: &Rc<Self>) {
        if let Some(t) = self.settle.take() {
            t.remove();
        }
        let weak = Rc::downgrade(self);
        let t = glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
            let Some(ed) = weak.upgrade() else { return };
            ed.settle.take();
            let pages: Vec<_> = ed
                .state
                .borrow()
                .cache
                .iter()
                .filter(|(_, c)| c.patched && !c.stale)
                .map(|(&p, c)| (p, c.scale, c.svg.clone(), c.svg_hash))
                .collect();
            for (p, scale, svg, h) in pages {
                ed.render_whole_page(p, scale, svg, h);
            }
        });
        self.settle.set(Some(t));
    }

    /// Re-render after the model changed and redraw/resize pages.
    fn after_model_change(self: &Rc<Self>, pages_changed: bool) {
        self.refresh_counts();
        let mut st = self.state.borrow_mut();
        st.model_gen += 1;
        for c in st.cache.values_mut() {
            c.stale = true;
        }
        drop(st);
        if pages_changed {
            self.rebuild_pages();
        } else {
            self.resize_pages();
        }
    }

    /// Apply an edit at the caret (with the current selection, if any); on success,
    /// re-render and move the caret. The selection is consumed.
    fn edit(self: &Rc<Self>, f: impl FnOnce(&mut Document, Pos, Option<Range>) -> EditResult) {
        let sel = self.selection();
        let result = {
            let mut st = self.state.borrow_mut();
            let Some(pos) = st.caret else { return };
            let Some(l) = st.loaded.as_mut() else { return };
            let before = l.doc.page_count();
            f(&mut l.doc, pos, sel).map(|p| (p, before != l.doc.page_count()))
        };
        match result {
            Ok((new_pos, pages_changed)) => {
                self.after_model_change(pages_changed);
                let p = new_pos.or(self.state.borrow().caret);
                self.set_caret(p);
                self.scroll_to_caret();
            }
            Err(e) => self.toast(&format!("편집할 수 없습니다: {e}")),
        }
    }

    fn insert(self: &Rc<Self>, text: &str) {
        let text = text.to_owned();
        let (pending, after) = {
            let mut st = self.state.borrow_mut();
            let after = st.after_object.take().filter(|a| Some(*a) == st.caret);
            (st.pending.take(), after)
        };
        self.edit(move |d, p, sel| match (sel, after, pending) {
            (Some(r), _, _) => d.replace_range(r, &text).map(Some),
            (None, Some(a), _) => d.insert_text_after_object(a, &text).map(Some),
            (None, None, Some(f)) => d.insert_text_formatted(p, &text, &f).map(Some),
            (None, None, None) => d.insert_text(p, &text).map(Some),
        });
    }

    /// Whether the caret is right after an object (and forget it: the edit moves on).
    fn take_after_object(&self) -> bool {
        let mut st = self.state.borrow_mut();
        let a = st.after_object.take();
        a.is_some() && a == st.caret
    }

    /// If an inline object sits at the caret's offset, put the caret after it.
    fn attach_after_object(&self) {
        let caret = {
            let mut st = self.state.borrow_mut();
            let (Some(l), Some(p)) = (&st.loaded, st.caret) else {
                return;
            };
            if l.doc.object_at(p, true).is_none() {
                return;
            }
            st.after_object = Some(p);
            st.caret
        };
        self.set_caret_keep_anchor(caret);
    }

    /// After putting in an object at the caret: the next typed text goes after it.
    fn mark_after_object(self: &Rc<Self>) {
        let caret = {
            let mut st = self.state.borrow_mut();
            st.after_object = st.caret;
            st.caret
        };
        self.set_caret_keep_anchor(caret);
        // the object came from a dialog: once it has closed (and put focus back on the
        // button that opened it), give the keyboard back to the page
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(ed) = weak.upgrade() {
                ed.focus_page();
            }
        });
    }

    /// Apply character formatting to the selection.
    /// The cells of a 셀 블록 (anchor and caret in different cells of one table), each as
    /// a range over its whole text. Hwp formats every selected cell, not just one.
    fn cell_block_ranges(&self) -> Option<Vec<Range>> {
        let st = self.state.borrow();
        let (a, p) = (st.anchor?, st.caret?);
        if Range::new(a, p).is_some() || !same_table(a, p) {
            return None;
        }
        st.loaded.as_ref()?.doc.cell_block_ranges(a, p).ok()
    }

    fn apply_format(self: &Rc<Self>, fmt: CharFormat) {
        if let Some(cells) = self.cell_block_ranges() {
            let result = {
                let mut st = self.state.borrow_mut();
                let Some(l) = st.loaded.as_mut() else { return };
                let before = l.doc.page_count();
                l.doc
                    .apply_char_format_ranges(&cells, &fmt)
                    .map(|_| before != l.doc.page_count())
            };
            return self.finish_format(result);
        }
        let Some(sel) = self.selection() else {
            // no selection: remember it for the next typed text, like Hancom
            {
                let mut st = self.state.borrow_mut();
                if st.caret.is_none() {
                    return;
                }
                let merged = merge(st.pending.take().unwrap_or_default(), fmt);
                st.pending = Some(merged);
            }
            self.sync_ribbon();
            self.focus_page();
            return;
        };
        let result = {
            let mut st = self.state.borrow_mut();
            let Some(l) = st.loaded.as_mut() else { return };
            let before = l.doc.page_count();
            l.doc
                .apply_char_format(sel, &fmt)
                .map(|_| before != l.doc.page_count())
        };
        self.finish_format(result);
    }

    /// Paragraphs touched by the selection, or the caret's paragraph.
    fn para_range(&self) -> Option<Range> {
        let st = self.state.borrow();
        let p = st.caret?;
        Some(Range::new(st.anchor.unwrap_or(p), p).unwrap_or(Range { start: p, end: p }))
    }

    /// Paragraph ranges a paragraph format applies to: every cell of a 셀 블록, else
    /// the paragraphs touched by the selection or the caret's paragraph.
    fn para_targets(&self) -> Option<Vec<Range>> {
        self.cell_block_ranges()
            .or_else(|| self.para_range().map(|r| vec![r]))
    }

    fn apply_align(self: &Rc<Self>, align: Align) {
        let Some(ranges) = self.para_targets() else {
            return;
        };
        let result = {
            let mut st = self.state.borrow_mut();
            let Some(l) = st.loaded.as_mut() else { return };
            let before = l.doc.page_count();
            l.doc
                .set_align_ranges(&ranges, align)
                .map(|_| before != l.doc.page_count())
        };
        self.finish_format(result);
    }

    fn apply_para(self: &Rc<Self>, fmt: ParaFormat) {
        let Some(ranges) = self.para_targets() else {
            return;
        };
        let result = {
            let mut st = self.state.borrow_mut();
            let Some(l) = st.loaded.as_mut() else { return };
            let before = l.doc.page_count();
            l.doc
                .set_para_format_ranges(&ranges, &fmt)
                .map(|_| before != l.doc.page_count())
        };
        self.finish_format(result);
    }

    // ------------------------------------------------------------------ clipboard

    fn copy(&self) -> bool {
        let Some(sel) = self.selection() else {
            return false;
        };
        let text = {
            let mut st = self.state.borrow_mut();
            let Some(l) = st.loaded.as_mut() else {
                return false;
            };
            match l.doc.copy(sel) {
                Ok(t) => t,
                Err(e) => {
                    drop(st);
                    self.toast(&format!("복사할 수 없습니다: {e}"));
                    return false;
                }
            }
        };
        self.win.clipboard().set_text(&text);
        true
    }

    fn cut(self: &Rc<Self>) {
        if self.copy() {
            self.edit(|d, _, sel| match sel {
                Some(r) => d.delete_range(r).map(Some),
                None => Ok(None),
            });
        }
    }

    /// Paste: our own formatted clipboard if the system clipboard still holds what we
    /// copied, otherwise the system clipboard's plain text.
    fn paste(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.win
            .clipboard()
            .read_text_async(gio::Cancellable::NONE, move |res| {
                let Some(ed) = weak.upgrade() else { return };
                let Ok(Some(text)) = res else {
                    ed.toast("클립보드에 붙여넣을 텍스트가 없습니다");
                    return;
                };
                let text = text.to_string();
                // a table copied as an object comes back as an object
                let object = {
                    let st = ed.state.borrow();
                    st.object_clip_text.as_deref() == Some(text.as_str())
                        && st
                            .loaded
                            .as_ref()
                            .is_some_and(|l| l.doc.clipboard_has_object())
                };
                if object {
                    if ed.in_table() {
                        ed.toast("표 안에는 표를 붙일 수 없습니다");
                    } else {
                        ed.edit(|d, p, _| d.paste_object(p).map(Some));
                    }
                    return;
                }
                let internal = ed
                    .state
                    .borrow()
                    .loaded
                    .as_ref()
                    .and_then(|l| l.doc.internal_clipboard_text());
                ed.edit(move |d, p, sel| {
                    if internal.as_deref() == Some(text.as_str()) {
                        d.paste_internal(p, sel).map(Some)
                    } else {
                        d.paste_text(p, sel, &text).map(Some)
                    }
                });
                ed.scroll_to_caret();
            });
    }

    /// Keep the caret visible.
    fn scroll_to_caret(self: &Rc<Self>) {
        let (rect, zoom) = {
            let st = self.state.borrow();
            (st.caret_rect, st.zoom)
        };
        let Some(r) = rect else { return };
        let Some(area) = self.areas.borrow().get(r.page as usize).cloned() else {
            return;
        };
        if area.width() == 0 {
            // pages were just rebuilt and are not laid out yet; positions would be wrong
            let weak = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                if let Some(ed) = weak.upgrade() {
                    ed.scroll_to_caret();
                }
            });
            return;
        }
        let Some(pt) = area.compute_point(
            &self.pages,
            &graphene::Point::new((r.x * zoom) as f32, (r.y * zoom) as f32),
        ) else {
            return;
        };
        let (y, h) = (pt.y() as f64, r.height * zoom);
        let adj = self.scroller.vadjustment();
        let margin = 40.0;
        if y < adj.value() + margin || y + h > adj.value() + adj.page_size() - margin {
            adj.set_value(
                (y - adj.page_size() / 3.0).clamp(adj.lower(), adj.upper() - adj.page_size()),
            );
        }
        let hadj = self.scroller.hadjustment();
        let x = pt.x() as f64;
        if x < hadj.value() || x > hadj.value() + hadj.page_size() - margin {
            hadj.set_value(
                (x - hadj.page_size() / 2.0).clamp(hadj.lower(), hadj.upper() - hadj.page_size()),
            );
        }
    }

    // ------------------------------------------------------------------ find / replace

    fn open_find(&self, replace: bool) {
        self.find.replace_row.set_visible(replace);
        self.find.bar.set_search_mode(true);
        let prefill = self.selection().and_then(|r| {
            let st = self.state.borrow();
            st.loaded
                .as_ref()?
                .doc
                .text_in(r)
                .filter(|t| t.chars().count() <= 40 && !t.contains('\t'))
        });
        if let Some(t) = prefill {
            self.find.query.set_text(&t);
        }
        self.find.query.grab_focus();
    }

    fn refresh_hits(&self) {
        let q = self.find.query.text().to_string();
        let case = self.find.case.is_active();
        let hits = self
            .state
            .borrow()
            .loaded
            .as_ref()
            .map(|l| l.doc.find_all(&q, case))
            .unwrap_or_default();
        let n = hits.len();
        {
            let mut st = self.state.borrow_mut();
            st.find_hits = hits;
            st.find_idx = None;
        }
        self.find.count.set_label(&if q.is_empty() {
            String::new()
        } else if n == 0 {
            "결과 없음".into()
        } else {
            format!("{n}개")
        });
    }

    /// Select the next (or previous) match after the caret.
    fn find_step(self: &Rc<Self>, forward: bool) {
        let (target, n) = {
            let st = self.state.borrow();
            let hits = &st.find_hits;
            if hits.is_empty() {
                return;
            }
            let n = hits.len();
            let i = match st.find_idx {
                Some(i) => {
                    if forward {
                        (i + 1) % n
                    } else {
                        (i + n - 1) % n
                    }
                }
                None => {
                    // first match after the caret in document order (find_all is ordered)
                    let after = st.caret.and_then(|c| {
                        hits.iter()
                            .position(|h| h.start == c || precedes(c, h.start))
                    });
                    if forward {
                        after.unwrap_or(0)
                    } else {
                        after.map_or(n - 1, |a| (a + n - 1) % n)
                    }
                }
            };
            (i, n)
        };
        let hit = self.state.borrow().find_hits[target];
        self.state.borrow_mut().find_idx = Some(target);
        self.state.borrow_mut().anchor = Some(hit.start);
        self.set_caret_keep_anchor(Some(hit.end));
        self.scroll_to_caret();
        self.find.count.set_label(&format!("{}/{n}", target + 1));
    }

    fn replace_one(self: &Rc<Self>) {
        let current = {
            let st = self.state.borrow();
            st.find_idx.and_then(|i| st.find_hits.get(i).copied())
        };
        if current.is_none() || current != self.selection() {
            self.find_step(true);
            return;
        }
        let with = self.find.with.text().to_string();
        self.edit(move |d, _, sel| match sel {
            Some(r) => d.replace_range(r, &with).map(Some),
            None => Ok(None),
        });
        self.refresh_hits();
        self.find_step(true);
    }

    fn replace_all(self: &Rc<Self>) {
        let q = self.find.query.text().to_string();
        let with = self.find.with.text().to_string();
        let case = self.find.case.is_active();
        let result = {
            let mut st = self.state.borrow_mut();
            let Some(l) = st.loaded.as_mut() else { return };
            let before = l.doc.page_count();
            l.doc
                .replace_all(&q, &with, case)
                .map(|n| (n, before != l.doc.page_count()))
        };
        match result {
            Ok((0, _)) => self.toast("바꿀 내용이 없습니다"),
            Ok((n, pages_changed)) => {
                self.after_model_change(pages_changed);
                let p = self.state.borrow().caret;
                self.set_caret(p);
                self.toast(&format!(
                    "{n}개를 바꿨습니다 (한 번에 실행 취소할 수 있습니다)"
                ));
            }
            Err(e) => self.toast(&format!("바꿀 수 없습니다: {e}")),
        }
        self.refresh_hits();
    }

    fn install_find(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.find.query.connect_search_changed(move |_| {
            if let Some(ed) = weak.upgrade() {
                ed.refresh_hits();
                ed.find_step(true);
            }
        });
        let weak = Rc::downgrade(self);
        self.find.query.connect_activate(move |_| {
            if let Some(ed) = weak.upgrade() {
                ed.find_step(true);
            }
        });
        let weak = Rc::downgrade(self);
        self.find.query.connect_previous_match(move |_| {
            if let Some(ed) = weak.upgrade() {
                ed.find_step(false);
            }
        });
        let weak = Rc::downgrade(self);
        self.find.query.connect_next_match(move |_| {
            if let Some(ed) = weak.upgrade() {
                ed.find_step(true);
            }
        });
        let weak = Rc::downgrade(self);
        self.find.case.connect_toggled(move |_| {
            if let Some(ed) = weak.upgrade() {
                ed.refresh_hits();
            }
        });
        let weak = Rc::downgrade(self);
        self.find.with.connect_activate(move |_| {
            if let Some(ed) = weak.upgrade() {
                ed.replace_one();
            }
        });
        let weak = Rc::downgrade(self);
        self.find.bar.connect_search_mode_enabled_notify(move |b| {
            if let Some(ed) = weak.upgrade() {
                if !b.is_search_mode() {
                    ed.focus_page();
                }
            }
        });
    }

    fn finish_format(self: &Rc<Self>, result: Result<bool, hwpedit_engine::EngineError>) {
        match result {
            Ok(pages_changed) => {
                self.after_model_change(pages_changed);
                let p = self.state.borrow().caret;
                self.set_caret_keep_anchor(p);
                self.focus_page();
            }
            Err(e) => {
                self.toast(&format!("서식을 적용할 수 없습니다: {e}"));
                self.sync_ribbon();
            }
        }
    }

    /// Toggle a boolean property based on the formatting at the selection start.
    fn toggle_format(
        self: &Rc<Self>,
        which: fn(&mut CharFormat, bool),
        current: fn(&hwpedit_engine::CharState) -> bool,
    ) {
        let state = {
            let st = self.state.borrow();
            let at = match (self.selection(), st.caret) {
                (Some(r), _) => Some(r.start.with_offset(r.start.offset() + 1)),
                (None, p) => p,
            };
            match (&st.loaded, at) {
                (Some(l), Some(p)) => l.doc.char_state(p).ok().map(|mut c| {
                    if let Some(f) = &st.pending {
                        overlay(&mut c, f);
                    }
                    c
                }),
                _ => None,
            }
        };
        let mut f = CharFormat::default();
        which(&mut f, !state.as_ref().is_some_and(current));
        self.apply_format(f);
    }

    fn focus_page(&self) {
        let page = self.state.borrow().caret_rect.map(|r| r.page);
        if let Some(a) = page.and_then(|p| self.areas.borrow().get(p as usize).cloned()) {
            a.grab_focus();
        }
    }

    fn undo_redo(self: &Rc<Self>, undo: bool) {
        let result = {
            let mut st = self.state.borrow_mut();
            let caret = st.caret.unwrap_or(Pos::Body {
                section: 0,
                para: 0,
                offset: 0,
            });
            let Some(l) = st.loaded.as_mut() else { return };
            let before = l.doc.page_count();
            let r = if undo {
                l.doc.undo(caret)
            } else {
                l.doc.redo(caret)
            };
            r.map(|p| (p, before != l.doc.page_count()))
        };
        match result {
            Ok((Some(p), pages_changed)) => {
                self.after_model_change(pages_changed);
                self.set_caret(Some(p));
            }
            Ok((None, _)) => {}
            Err(e) => self.toast(&format!(
                "{}할 수 없습니다: {e}",
                if undo {
                    "실행 취소"
                } else {
                    "다시 실행"
                }
            )),
        }
        self.refresh_chrome();
    }

    fn select_all(&self) {
        let range = {
            let st = self.state.borrow();
            let (Some(l), Some(p)) = (&st.loaded, st.caret) else {
                return;
            };
            l.doc.container_range(p)
        };
        self.state.borrow_mut().anchor = Some(range.start);
        self.set_caret_keep_anchor(Some(range.end));
    }

    /// Navigation key: move or (with Shift) extend. Without Shift, Left/Right on a
    /// selection collapse it to its start/end like most editors.
    fn move_caret(self: &Rc<Self>, extend: bool, f: impl FnOnce(&Document, Pos) -> Option<Pos>) {
        let target = {
            let st = self.state.borrow();
            match (&st.loaded, st.caret) {
                (Some(l), Some(p)) => f(&l.doc, p),
                _ => None,
            }
        };
        if let Some(p) = target {
            self.place(Some(p), extend);
            self.scroll_to_caret();
        }
    }

    /// ←/→ next to an inline object (수식, 각주 …): it takes no text offset, so the caret
    /// stops on both sides of it — before (offset k) and after (offset k, `after_object`).
    /// Returns false when there is no object to step onto (the usual move applies).
    fn step_over_object(self: &Rc<Self>, right: bool) -> bool {
        let target = {
            let st = self.state.borrow();
            let (Some(l), Some(p)) = (&st.loaded, st.caret) else {
                return false;
            };
            let after = st.after_object == Some(p);
            let doc = &l.doc;
            match (right, after) {
                // before an object -> after it
                (true, false) if doc.object_at(p, false).is_some() => Some((p, true)),
                // after an object -> before it
                (false, true) => Some((p, false)),
                // from the letter after an object, ← lands right after the object
                (false, false)
                    if p.offset() > 0
                        && doc.object_at(p.with_offset(p.offset() - 1), true).is_some() =>
                {
                    Some((p.with_offset(p.offset() - 1), true))
                }
                _ => None,
            }
        };
        let Some((pos, after)) = target else {
            return false;
        };
        self.place(Some(pos), false);
        if after {
            self.state.borrow_mut().after_object = Some(pos);
            self.set_caret_keep_anchor(Some(pos));
        }
        self.scroll_to_caret();
        true
    }

    /// Up/Down: hit-test above/below the caret line, stepping over the line gap
    /// (with 160% spacing the point just below the caret hits the same line again).
    fn move_vertical(self: &Rc<Self>, down: bool, extend: bool) {
        let Some(r) = self.state.borrow().caret_rect else {
            return;
        };
        let target = {
            let st = self.state.borrow();
            let Some(l) = st.loaded.as_ref() else { return };
            (1..=(3.0 * r.height.max(8.0) / 2.0) as i32).find_map(|i| {
                let step = 2.0 * i as f64;
                let y = if down {
                    r.y + r.height + step
                } else {
                    r.y - step
                };
                let Ok(Hit::Editable(p)) = l.doc.hit_test(r.page, r.x, y) else {
                    return None;
                };
                let to = l.doc.caret_rect(p).ok()?;
                let moved = if down {
                    to.y > r.y + 1.0
                } else {
                    to.y < r.y - 1.0
                };
                (Some(p) != st.caret && moved).then_some(p)
            })
        };
        if let Some(p) = target {
            self.place(Some(p), extend);
            self.scroll_to_caret();
        }
    }

    fn on_key(self: &Rc<Self>, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        if self.state.borrow().caret.is_none() || !self.state.borrow().preedit.is_empty() {
            return glib::Propagation::Proceed;
        }
        // F7 is Hwp's 편집 용지, but GtkWindow keeps F7 for caret browsing and takes it
        // before application accelerators: claim it here, in the capture phase
        if key == gdk::Key::F7
            && !mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK)
        {
            WidgetExt::activate_action(&self.win, "win.page-setup", None).ok();
            return glib::Propagation::Stop;
        }
        // F6 (스타일) likewise: GtkWindow uses it to move focus between panes
        if key == gdk::Key::F6
            && !mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK)
        {
            WidgetExt::activate_action(&self.win, "win.style-dialog", None).ok();
            return glib::Propagation::Stop;
        }
        // a table selected as an object takes Delete/Backspace/Esc/Ctrl+C/Ctrl+X
        // copy out first: an `if let` on the borrow would hold it through the edits below
        let selected = self.state.borrow().table_selected;
        if let Some(t) = selected {
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            match key.to_lower() {
                gdk::Key::Delete | gdk::Key::KP_Delete | gdk::Key::BackSpace => {
                    self.select_table(None);
                    self.edit(move |d, _, _| d.delete_table(t).map(Some));
                    return glib::Propagation::Stop;
                }
                gdk::Key::Escape => {
                    self.select_table(None);
                    return glib::Propagation::Stop;
                }
                gdk::Key::c if ctrl => {
                    self.copy_table();
                    return glib::Propagation::Stop;
                }
                gdk::Key::x if ctrl => {
                    if self.copy_table() {
                        self.select_table(None);
                        self.edit(move |d, _, _| d.delete_table(t).map(Some));
                    }
                    return glib::Propagation::Stop;
                }
                _ => self.select_table(None),
            }
        }
        if mods.contains(gdk::ModifierType::CONTROL_MASK)
            && !mods.contains(gdk::ModifierType::ALT_MASK)
        {
            // handled here rather than as app accelerators so entries in the ribbon and
            // find bar keep their own copy/paste/select-all
            let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
            // Hwp 2022's 서식 menu: 개요 Ctrl+Insert, 문단 번호 Ctrl+Shift+Insert,
            // 글머리표 Ctrl+Shift+Delete, 한 수준 증가/감소 Ctrl+Num -/+
            let list_action = match key.to_lower() {
                gdk::Key::Insert | gdk::Key::KP_Insert if shift => Some("win.list-number"),
                gdk::Key::Insert | gdk::Key::KP_Insert => Some("win.outline"),
                gdk::Key::Delete | gdk::Key::KP_Delete if shift => Some("win.list-bullet"),
                gdk::Key::KP_Subtract => Some("win.level-up"),
                gdk::Key::KP_Add => Some("win.level-down"),
                _ => None,
            };
            if let Some(action) = list_action {
                WidgetExt::activate_action(&self.win, action, None).ok();
                return glib::Propagation::Stop;
            }
            match key.to_lower() {
                gdk::Key::c => {
                    self.copy();
                }
                gdk::Key::x => self.cut(),
                gdk::Key::v => self.paste(),
                gdk::Key::a => self.select_all(),
                gdk::Key::Return | gdk::Key::KP_Enter => self.page_break(),
                // in a table: grow/shrink the column or row, the table with it (Hwp: Ctrl+arrows)
                k @ (gdk::Key::Left | gdk::Key::Right | gdk::Key::Up | gdk::Key::Down)
                    if self.in_table() =>
                {
                    self.resize_key(k, Resize::Table)
                }
                _ => return glib::Propagation::Proceed,
            }
            return glib::Propagation::Stop;
        }
        if mods.contains(gdk::ModifierType::ALT_MASK) {
            // in a table: move the border, table size unchanged (Hwp: Alt+arrows)
            if let k @ (gdk::Key::Left | gdk::Key::Right | gdk::Key::Up | gdk::Key::Down) = key {
                if self.in_table() {
                    self.resize_key(k, Resize::Border);
                    return glib::Propagation::Stop;
                }
            }
            return glib::Propagation::Proceed;
        }
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let sel = self.selection();
        match key {
            gdk::Key::BackSpace => {
                // right after a 수식/각주: Backspace removes the object, as in Hwp
                let after = self.take_after_object();
                let removed_in_line = Rc::new(Cell::new(false));
                let flag = removed_in_line.clone();
                self.edit(move |d, p, sel| match sel {
                    Some(r) => d.delete_range(r).map(Some),
                    None if after && d.object_at(p, true).is_some() => {
                        let c = d.object_at(p, true).unwrap_or_default();
                        flag.set(true);
                        d.delete_object(p, c).map(|_| Some(p))
                    }
                    None if p.offset() > 0 => {
                        let at = p.with_offset(p.offset() - 1);
                        flag.set(true);
                        d.delete_text(at, 1).map(|_| Some(at))
                    }
                    None => d.merge_with_previous(p),
                });
                // what was removed came after any object at the new caret offset:
                // the caret stays after that object, not before it
                if removed_in_line.get() {
                    self.attach_after_object();
                }
            }
            gdk::Key::Delete | gdk::Key::KP_Delete => {
                let after = self.take_after_object();
                self.edit(move |d, p, sel| match sel {
                    Some(r) => d.delete_range(r).map(Some),
                    // just before a 수식/각주: Delete removes the object
                    None if !after && d.object_at(p, false).is_some() => {
                        let c = d.object_at(p, false).unwrap_or_default();
                        d.delete_object(p, c).map(|_| Some(p))
                    }
                    None if p.offset() < d.para_len(p) => d.delete_text(p, 1).map(|_| Some(p)),
                    None => match next_para(p) {
                        Some(n) if para_index(n) < d.para_count(n) => d.merge_with_previous(n),
                        _ => Ok(Some(p)),
                    },
                });
                if after && self.selection().is_none() {
                    self.attach_after_object();
                }
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                let after = {
                    let mut st = self.state.borrow_mut();
                    st.after_object.take().filter(|a| Some(*a) == st.caret)
                };
                self.edit(move |d, p, sel| match (sel, after) {
                    (Some(r), _) => {
                        let p = d.delete_range(r)?;
                        d.split_paragraph(p).map(Some)
                    }
                    (None, Some(a)) => d.split_paragraph_after_object(a).map(Some),
                    (None, None) => d.split_paragraph(p).map(Some),
                })
            }
            gdk::Key::Left if sel.is_none() && !shift && self.step_over_object(false) => {}
            gdk::Key::Right if sel.is_none() && !shift && self.step_over_object(true) => {}
            gdk::Key::Left => match sel.filter(|_| !shift) {
                Some(r) => self.place(Some(r.start), false),
                None => self.move_caret(shift, |d, p| {
                    if p.offset() > 0 {
                        Some(p.with_offset(p.offset() - 1))
                    } else {
                        prev_para(p).map(|q| q.with_offset(d.para_len(q)))
                    }
                }),
            },
            gdk::Key::Right => match sel.filter(|_| !shift) {
                Some(r) => self.place(Some(r.end), false),
                None => self.move_caret(shift, |d, p| {
                    if p.offset() < d.para_len(p) {
                        Some(p.with_offset(p.offset() + 1))
                    } else {
                        next_para(p).filter(|n| para_index(*n) < d.para_count(*n))
                    }
                }),
            },
            gdk::Key::Home => self.move_caret(shift, |_, p| Some(p.with_offset(0))),
            gdk::Key::End => {
                self.move_caret(shift, |d, p| Some(p.with_offset(d.para_len(p))));
                // the end of a paragraph that ends in a 수식 is after it
                if !shift {
                    self.attach_after_object();
                }
            }
            gdk::Key::Up => self.move_vertical(false, shift),
            gdk::Key::Down => self.move_vertical(true, shift),
            gdk::Key::Escape if sel.is_none() && self.in_table() => {
                // Esc in a cell selects the whole table as an object (Hwp, HOP)
                let p = self.state.borrow().caret;
                self.select_table(p);
            }
            gdk::Key::Escape => {
                if sel.is_some() {
                    let p = self.state.borrow().caret;
                    self.set_caret(p);
                } else {
                    self.set_caret(None);
                }
            }
            // in a table, Tab goes to the next cell and adds a row after the last one (as in Hwp)
            gdk::Key::Tab if self.in_table() => self.edit(|d, p, _| d.tab_forward(p).map(Some)),
            gdk::Key::ISO_Left_Tab if self.in_table() => {
                self.move_caret(false, |d, p| d.next_cell(p, false))
            }
            gdk::Key::Tab => self.insert("\t"),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    fn install_input(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.set_im_context(Some(&self.im));
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, mods| match weak.upgrade() {
            Some(ed) => ed.on_key(key, mods),
            None => glib::Propagation::Proceed,
        });
        // on the page view, not the window: typing in ribbon entries must not reach the document
        self.scroller.add_controller(keys);

        let weak = Rc::downgrade(self);
        self.im.connect_commit(move |_, text| {
            ime_log(format_args!("commit {text:?}"));
            if let Some(ed) = weak.upgrade() {
                ed.state.borrow_mut().preedit.clear();
                if ed.state.borrow().caret.is_some() {
                    ed.insert(text);
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.im.connect_preedit_changed(move |im| {
            if let Some(ed) = weak.upgrade() {
                let (s, _, _) = im.preedit_string();
                ime_log(format_args!("preedit {s:?}"));
                ed.state.borrow_mut().preedit = s.to_string();
                let page = ed.state.borrow().caret_rect.map(|r| r.page);
                if let Some(a) = page.and_then(|p| ed.areas.borrow().get(p as usize).cloned()) {
                    a.queue_draw();
                }
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let im = self.im.clone();
        focus.connect_enter(move |_| im.focus_in());
        let im = self.im.clone();
        focus.connect_leave(move |_| im.focus_out());
        self.scroller.add_controller(focus);
    }

    fn install_drop(self: &Rc<Self>) {
        let target = gtk::DropTarget::new(gio::File::static_type(), gdk::DragAction::COPY);
        let weak = Rc::downgrade(self);
        target.connect_drop(move |_, value, _, _| {
            let (Some(ed), Ok(file)) = (weak.upgrade(), value.get::<gio::File>()) else {
                return false;
            };
            let Some(path) = file.path() else {
                return false;
            };
            crate::open_path(&ed, &path);
            true
        });
        self.win.add_controller(target);
    }

    // ------------------------------------------------------------------ actions

    fn install_actions(self: &Rc<Self>) {
        type Handler = Box<dyn Fn(&Rc<Editor>)>;
        let add = |name: &str, f: Handler| {
            let a = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(ed) = weak.upgrade() {
                    f(&ed);
                }
            });
            self.win.add_action(&a);
        };
        add("save", Box::new(|ed| ed.save_in_place(|_| {})));
        add("save-as", Box::new(|ed| ed.save_as()));
        add("zoom-in", Box::new(|ed| ed.zoom_step(1)));
        add("zoom-out", Box::new(|ed| ed.zoom_step(-1)));
        add("zoom-reset", Box::new(|ed| ed.set_zoom(1.0)));
        add(
            "support",
            Box::new(|ed| ed.alert("지원 범위와 제약", crate::SUPPORT_TEXT)),
        );
        add("undo", Box::new(|ed| ed.undo_redo(true)));
        add("redo", Box::new(|ed| ed.undo_redo(false)));
        add("select-all", Box::new(|ed| ed.select_all()));
        add(
            "copy",
            Box::new(|ed| {
                ed.copy();
            }),
        );
        add("cut", Box::new(|ed| ed.cut()));
        add("paste", Box::new(|ed| ed.paste()));
        add("find", Box::new(|ed| ed.open_find(false)));
        add("replace", Box::new(|ed| ed.open_find(true)));
        add("find-next", Box::new(|ed| ed.find_step(true)));
        add("find-prev", Box::new(|ed| ed.find_step(false)));
        add("replace-one", Box::new(|ed| ed.replace_one()));
        add("replace-all", Box::new(|ed| ed.replace_all()));
        add("export-pdf", Box::new(|ed| ed.export_pdf()));
        add("char-shape", Box::new(|ed| ed.char_shape_dialog()));
        add("para-shape", Box::new(|ed| ed.para_shape_dialog()));
        add("print", Box::new(|ed| ed.print()));
        add("zoom-fit-page", Box::new(|ed| ed.zoom_fit(true)));
        add("zoom-fit-width", Box::new(|ed| ed.zoom_fit(false)));
        add("page-break", Box::new(|ed| ed.page_break()));
        add(
            "insert-table",
            Box::new(|ed| {
                let (r, c) = (
                    ed.ribbon.table_rows.value() as u16,
                    ed.ribbon.table_cols.value() as u16,
                );
                ed.edit(move |d, p, _| d.create_table(p, r, c).map(Some));
                ed.focus_page();
            }),
        );
        add(
            "table-row-above",
            Box::new(|ed| ed.edit(|d, p, _| d.insert_row(p, false).map(Some))),
        );
        add(
            "table-row-below",
            Box::new(|ed| ed.edit(|d, p, _| d.insert_row(p, true).map(Some))),
        );
        add(
            "table-col-left",
            Box::new(|ed| ed.edit(|d, p, _| d.insert_col(p, false).map(Some))),
        );
        add(
            "table-col-right",
            Box::new(|ed| ed.edit(|d, p, _| d.insert_col(p, true).map(Some))),
        );
        add(
            "table-row-delete",
            Box::new(|ed| ed.edit(|d, p, _| d.delete_row(p).map(Some))),
        );
        add(
            "table-col-delete",
            Box::new(|ed| ed.edit(|d, p, _| d.delete_col(p).map(Some))),
        );
        add(
            "cell-split",
            Box::new(|ed| ed.edit(|d, p, _| d.split_cell(p).map(Some))),
        );
        add(
            "table-delete",
            Box::new(|ed| ed.edit(|d, p, _| d.delete_table(p).map(Some))),
        );
        add(
            "table-col-narrow",
            Box::new(|ed| ed.resize_key(gdk::Key::Left, Resize::Table)),
        );
        add(
            "table-col-wide",
            Box::new(|ed| ed.resize_key(gdk::Key::Right, Resize::Table)),
        );
        add(
            "table-row-short",
            Box::new(|ed| ed.resize_key(gdk::Key::Up, Resize::Table)),
        );
        add(
            "table-row-tall",
            Box::new(|ed| ed.resize_key(gdk::Key::Down, Resize::Table)),
        );
        add(
            "table-col-equal",
            Box::new(|ed| ed.edit(|d, p, _| d.equalize_columns(p).map(Some))),
        );
        add(
            "table-row-equal",
            Box::new(|ed| ed.edit(|d, p, _| d.equalize_rows(p).map(Some))),
        );
        add(
            "table-copy",
            Box::new(|ed| {
                ed.copy_table();
            }),
        );
        add(
            "table-select",
            Box::new(|ed| {
                let p = ed.state.borrow().caret;
                ed.select_table(p);
            }),
        );
        add("row-col-insert", Box::new(|ed| ed.row_col_dialog(true)));
        add("row-col-delete", Box::new(|ed| ed.row_col_dialog(false)));
        add(
            "delete",
            Box::new(|ed| {
                ed.edit(|d, _, sel| match sel {
                    Some(r) => d.delete_range(r).map(Some),
                    None => Ok(None),
                })
            }),
        );
        add("font-bigger", Box::new(|ed| ed.font_size_step(1.0)));
        add("font-smaller", Box::new(|ed| ed.font_size_step(-1.0)));
        add("spacing-up", Box::new(|ed| ed.line_spacing_step(10)));
        add("spacing-down", Box::new(|ed| ed.line_spacing_step(-10)));
        add("align-left", Box::new(|ed| ed.apply_align(Align::Left)));
        add("align-center", Box::new(|ed| ed.apply_align(Align::Center)));
        add("align-right", Box::new(|ed| ed.apply_align(Align::Right)));
        add(
            "align-justify",
            Box::new(|ed| ed.apply_align(Align::Justify)),
        );
        add(
            "align-distribute",
            Box::new(|ed| ed.apply_align(Align::Distribute)),
        );
        add("align-split", Box::new(|ed| ed.apply_align(Align::Split)));
        let zoom = gio::SimpleAction::new("zoom-set", Some(glib::VariantTy::INT32));
        let weak = Rc::downgrade(self);
        zoom.connect_activate(move |_, v| {
            if let (Some(ed), Some(pct)) = (weak.upgrade(), v.and_then(|v| v.get::<i32>())) {
                ed.set_zoom(pct as f64 / 100.0);
            }
        });
        self.win.add_action(&zoom);
        // Hwp 2022 controls shown in the ribbon before we support them
        let not_yet = gio::SimpleAction::new("not-yet", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        not_yet.connect_activate(move |_, v| {
            if let (Some(ed), Some(name)) = (weak.upgrade(), v.and_then(|v| v.get::<String>())) {
                ed.toast(&format!("{name}: 아직 지원하지 않는 기능입니다"));
            }
        });
        self.win.add_action(&not_yet);
        add(
            "cell-merge",
            Box::new(|ed| {
                let anchor = ed.state.borrow().anchor;
                match anchor {
                    Some(a) => ed.edit(move |d, p, _| d.merge_cells(a, p).map(Some)),
                    None => ed.toast("합칠 셀을 끌어서 고르세요"),
                }
            }),
        );
        add(
            "bold",
            Box::new(|ed| ed.toggle_format(|f, v| f.bold = Some(v), |c| c.bold)),
        );
        add(
            "italic",
            Box::new(|ed| ed.toggle_format(|f, v| f.italic = Some(v), |c| c.italic)),
        );
        add(
            "underline",
            Box::new(|ed| ed.toggle_format(|f, v| f.underline = Some(v), |c| c.underline)),
        );
    }

    fn install_ribbon(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.zoom_scale.connect_value_changed(move |s| {
            if let Some(ed) = weak.upgrade() {
                if !ed.syncing.get() {
                    ed.set_zoom(s.value() / 100.0);
                }
            }
        });
        let rb = &self.ribbon;
        let bind_toggle = |b: &gtk::ToggleButton, set: fn(&mut CharFormat, bool)| {
            let weak = Rc::downgrade(self);
            b.connect_toggled(move |b| {
                let Some(ed) = weak.upgrade() else { return };
                if ed.syncing.get() {
                    return;
                }
                let mut f = CharFormat::default();
                set(&mut f, b.is_active());
                ed.apply_format(f);
            });
        };
        bind_toggle(&rb.bold, |f, v| f.bold = Some(v));
        bind_toggle(&rb.italic, |f, v| f.italic = Some(v));
        bind_toggle(&rb.underline, |f, v| f.underline = Some(v));
        bind_toggle(&rb.strike, |f, v| f.strike = Some(v));

        let weak = Rc::downgrade(self);
        rb.size.connect_value_changed(move |s| {
            let Some(ed) = weak.upgrade() else { return };
            if !ed.syncing.get() {
                ed.apply_format(CharFormat {
                    size_pt: Some(s.value()),
                    ..Default::default()
                });
            }
        });
        let weak = Rc::downgrade(self);
        rb.style.connect_selected_notify(move |dd| {
            let Some(ed) = weak.upgrade() else { return };
            if !ed.syncing.get() && dd.selected() != gtk::INVALID_LIST_POSITION {
                WidgetExt::activate_action(
                    &ed.win,
                    "win.style",
                    Some(&(dd.selected() as i32).to_variant()),
                )
                .ok();
            }
        });
        let weak = Rc::downgrade(self);
        rb.font.connect_selected_notify(move |_| {
            let Some(ed) = weak.upgrade() else { return };
            if !ed.syncing.get() {
                let font = ed.ribbon.selected_font();
                ed.apply_format(CharFormat {
                    font,
                    ..Default::default()
                });
            }
        });
        let weak = Rc::downgrade(self);
        rb.color.connect_rgba_notify(move |c| {
            let Some(ed) = weak.upgrade() else { return };
            if !ed.syncing.get() {
                let v = c.rgba();
                let hex = format!(
                    "#{:02x}{:02x}{:02x}",
                    (v.red() * 255.0).round() as u8,
                    (v.green() * 255.0).round() as u8,
                    (v.blue() * 255.0).round() as u8
                );
                ed.apply_format(CharFormat {
                    color: Some(hex),
                    ..Default::default()
                });
            }
        });
        let bind_spin = |s: &gtk::SpinButton, make: fn(f64) -> ParaFormat| {
            let weak = Rc::downgrade(self);
            s.connect_value_changed(move |s| {
                let Some(ed) = weak.upgrade() else { return };
                if !ed.syncing.get() {
                    ed.apply_para(make(s.value()));
                }
            });
        };
        bind_spin(&rb.line_spacing, |v| ParaFormat {
            line_spacing_pct: Some(v.round() as i32),
            ..Default::default()
        });
        // 「찾을 내용」 in the menu row: Enter finds the next match
        let weak = Rc::downgrade(self);
        rb.find_entry.connect_activate(move |e| {
            let Some(ed) = weak.upgrade() else { return };
            let q = e.text().to_string();
            if q.is_empty() {
                return;
            }
            ed.find.replace_row.set_visible(false);
            ed.find.bar.set_search_mode(true);
            ed.find.query.set_text(&q);
            ed.refresh_hits();
            ed.find_step(true);
        });
        for (i, b) in rb.align.iter().enumerate() {
            let weak = Rc::downgrade(self);
            b.connect_toggled(move |b| {
                let Some(ed) = weak.upgrade() else { return };
                if b.is_active() && !ed.syncing.get() {
                    ed.apply_align(ALIGNS[i]);
                }
            });
        }
    }

    fn zoom_step(&self, dir: i32) {
        let z = self.state.borrow().zoom;
        let next = if dir > 0 {
            ZOOM_STEPS.iter().copied().find(|s| *s > z + 1e-6)
        } else {
            ZOOM_STEPS.iter().rev().copied().find(|s| *s < z - 1e-6)
        };
        if let Some(n) = next {
            self.set_zoom(n);
        }
    }

    fn set_zoom(&self, z: f64) {
        {
            let mut st = self.state.borrow_mut();
            st.zoom = z;
            st.cache.clear();
        }
        self.resize_pages();
        let p = self.state.borrow().caret;
        self.set_caret(p);
    }

    /// 글자 모양: Hwp's dialog, limited to what the engine can apply.
    fn char_shape_dialog(self: &Rc<Self>) {
        let cs = {
            let st = self.state.borrow();
            let (Some(l), Some(p)) = (&st.loaded, st.caret) else {
                return;
            };
            let at = match self.selection() {
                Some(r) => r.start.with_offset(r.start.offset() + 1),
                None => p,
            };
            let mut c = match l.doc.char_state(at) {
                Ok(c) => c,
                Err(e) => {
                    drop(st);
                    self.toast(&e.to_string());
                    return;
                }
            };
            if let Some(f) = &st.pending {
                overlay(&mut c, f);
            }
            c
        };
        let size = gtk::SpinButton::with_range(1.0, 300.0, 0.5);
        size.set_digits(1);
        size.set_value(cs.size_pt);
        let fonts = font_families(&self.scroller);
        let names: Vec<&str> = fonts.iter().map(String::as_str).collect();
        let font = gtk::DropDown::from_strings(&names);
        font.set_enable_search(true);
        font.set_expression(Some(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            None::<gtk::Expression>,
            "string",
        )));
        if let Some(i) = fonts.iter().position(|f| *f == cs.font) {
            font.set_selected(i as u32);
        }
        let attr = |markup: &str, tip: &str, on: bool| {
            let l = gtk::Label::new(None);
            l.set_markup(markup);
            let b = gtk::ToggleButton::builder()
                .child(&l)
                .tooltip_text(tip)
                .active(on)
                .build();
            b.add_css_class("attr");
            b
        };
        let bold = attr("<b>가</b>", "진하게", cs.bold);
        let italic = attr("<i>가</i>", "기울임", cs.italic);
        let underline = attr("<u>가</u>", "밑줄", cs.underline);
        let strike = attr("<s>가</s>", "취소선", cs.strike);
        let color = gtk::ColorDialogButton::new(Some(
            gtk::ColorDialog::builder().with_alpha(false).build(),
        ));
        if let Ok(c) = gdk::RGBA::parse(cs.color.as_str()) {
            color.set_rgba(&c);
        }
        let preview = gtk::Label::new(None);
        preview.add_css_class("char-preview");
        preview.set_size_request(-1, 72);
        let update = {
            let (font, size, bold, italic, underline, strike, color, preview) = (
                font.clone(),
                size.clone(),
                bold.clone(),
                italic.clone(),
                underline.clone(),
                strike.clone(),
                color.clone(),
                preview.clone(),
            );
            move || {
                let fam = font
                    .selected_item()
                    .and_downcast::<gtk::StringObject>()
                    .map(|s| s.string().to_string())
                    .unwrap_or_default();
                let c = color.rgba();
                let m = format!(
                    "<span font_family=\"{}\" size=\"{}pt\" weight=\"{}\" style=\"{}\" underline=\"{}\" strikethrough=\"{}\" foreground=\"#{:02x}{:02x}{:02x}\">한글Eng123漢字あいう</span>",
                    glib::markup_escape_text(&fam),
                    size.value().clamp(6.0, 40.0),
                    if bold.is_active() { "bold" } else { "normal" },
                    if italic.is_active() { "italic" } else { "normal" },
                    if underline.is_active() { "single" } else { "none" },
                    strike.is_active(),
                    (c.red() * 255.0) as u8,
                    (c.green() * 255.0) as u8,
                    (c.blue() * 255.0) as u8
                );
                preview.set_markup(&m);
            }
        };
        update();
        let u = Rc::new(update);
        for b in [&bold, &italic, &underline, &strike] {
            let u = u.clone();
            b.connect_toggled(move |_| u());
        }
        let u2 = u.clone();
        size.connect_value_changed(move |_| u2());
        let u3 = u.clone();
        font.connect_selected_notify(move |_| u3());
        let u4 = u.clone();
        color.connect_rgba_notify(move |_| u4());

        let grid = gtk::Grid::builder()
            .row_spacing(10)
            .column_spacing(12)
            .build();
        let cap = |t: &str| gtk::Label::builder().label(t).xalign(0.0).build();
        grid.attach(&cap("기준 크기"), 0, 0, 1, 1);
        let size_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        size_row.append(&size);
        size_row.append(&gtk::Label::new(Some("pt")));
        grid.attach(&size_row, 1, 0, 1, 1);
        grid.attach(&cap("글꼴"), 0, 1, 1, 1);
        grid.attach(&font, 1, 1, 1, 1);
        grid.attach(&cap("속성"), 0, 2, 1, 1);
        let attrs = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for b in [&bold, &italic, &underline, &strike] {
            attrs.append(b);
        }
        grid.attach(&attrs, 1, 2, 1, 1);
        grid.attach(&cap("글자 색"), 0, 3, 1, 1);
        grid.attach(&color, 1, 3, 1, 1);
        let frame = gtk::Frame::builder().child(&preview).build();
        grid.attach(&frame, 0, 4, 2, 1);

        let d = adw::AlertDialog::new(Some("글자 모양"), None);
        d.set_extra_child(Some(&grid));
        d.add_responses(&[("cancel", "취소"), ("apply", "설정")]);
        d.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("apply"));
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            let Some(ed) = weak.upgrade() else { return };
            if r != "apply" {
                return;
            }
            let c = color.rgba();
            let picked = font
                .selected_item()
                .and_downcast::<gtk::StringObject>()
                .map(|s| s.string().to_string());
            // only what changed, so mixed selections keep their other properties
            let mut f = CharFormat::default();
            let changed = |a: bool, b: bool| (a != b).then_some(a);
            f.bold = changed(bold.is_active(), cs.bold);
            f.italic = changed(italic.is_active(), cs.italic);
            f.underline = changed(underline.is_active(), cs.underline);
            f.strike = changed(strike.is_active(), cs.strike);
            f.size_pt = ((size.value() - cs.size_pt).abs() > 0.01).then(|| size.value());
            let hex = format!(
                "#{:02x}{:02x}{:02x}",
                (c.red() * 255.0).round() as u8,
                (c.green() * 255.0).round() as u8,
                (c.blue() * 255.0).round() as u8
            );
            f.color = (!hex.eq_ignore_ascii_case(&cs.color)).then_some(hex);
            f.font = picked.filter(|p| *p != cs.font);
            if f != CharFormat::default() {
                ed.apply_format(f);
            }
        });
        d.present(Some(&self.win));
    }

    /// 문단 모양: alignment, margins, first line, line spacing (all four kinds),
    /// spacing above/below and 줄 나눔.
    fn para_shape_dialog(self: &Rc<Self>) {
        let ps = {
            let st = self.state.borrow();
            let (Some(l), Some(p)) = (&st.loaded, st.caret) else {
                return;
            };
            match l.doc.para_state(p) {
                Some(ps) => ps,
                None => return,
            }
        };
        let align = [
            ("align-justify", "양쪽 정렬"),
            ("align-left", "왼쪽 정렬"),
            ("align-center", "가운데 정렬"),
            ("align-right", "오른쪽 정렬"),
        ]
        .map(|(i, t)| {
            gtk::ToggleButton::builder()
                .icon_name(icons::name(i))
                .tooltip_text(t)
                .build()
        });
        for b in &align[1..] {
            b.set_group(Some(&align[0]));
        }
        if let Some(i) = ALIGNS.iter().position(|a| *a == ps.align) {
            align[i].set_active(true);
        }
        let num = |v: f64, min: f64, max: f64| {
            let s = gtk::SpinButton::with_range(min, max, 0.5);
            s.set_digits(1);
            s.set_value(v);
            s
        };
        let left = num(ps.margin_left_pt, 0.0, 500.0);
        let right = num(ps.margin_right_pt, 0.0, 500.0);
        let first_amount = num(ps.indent_pt.abs(), 0.0, 200.0);
        let normal = gtk::CheckButton::with_label("보통");
        let indent = gtk::CheckButton::with_label("들여쓰기");
        let hanging = gtk::CheckButton::with_label("내어쓰기");
        indent.set_group(Some(&normal));
        hanging.set_group(Some(&normal));
        match ps.indent_pt {
            v if v > 0.0 => indent.set_active(true),
            v if v < 0.0 => hanging.set_active(true),
            _ => normal.set_active(true),
        }
        first_amount.set_sensitive(!normal.is_active());
        let fa = first_amount.clone();
        normal.connect_toggled(move |n| fa.set_sensitive(!n.is_active()));
        // 줄 간격 종류: 글자에 따라(%) / 고정 값 / 여백만 지정 / 최소 (pt)
        const KINDS: [(LineSpacingKind, &str); 4] = [
            (LineSpacingKind::Percent, "글자에 따라"),
            (LineSpacingKind::Fixed, "고정 값"),
            (LineSpacingKind::SpaceOnly, "여백만 지정"),
            (LineSpacingKind::Minimum, "최소"),
        ];
        let kind = gtk::DropDown::from_strings(&KINDS.map(|(_, t)| t));
        kind.set_selected(
            KINDS
                .iter()
                .position(|(k, _)| *k == ps.line_spacing.0)
                .unwrap_or(0) as u32,
        );
        let spacing = num(ps.line_spacing.1, 0.0, 500.0);
        let unit = gtk::Label::new(None);
        let set_unit = {
            let (spacing, unit) = (spacing.clone(), unit.clone());
            move |percent: bool| {
                if percent {
                    spacing.set_range(50.0, 500.0);
                    spacing.set_increments(10.0, 50.0);
                    spacing.set_digits(0);
                    unit.set_label("%");
                } else {
                    spacing.set_range(0.0, 500.0);
                    spacing.set_increments(1.0, 5.0);
                    spacing.set_digits(1);
                    unit.set_label("pt");
                }
            }
        };
        set_unit(ps.line_spacing.0 == LineSpacingKind::Percent);
        spacing.set_value(ps.line_spacing.1);
        {
            let spacing = spacing.clone();
            kind.connect_selected_notify(move |k| {
                let percent = k.selected() == 0;
                set_unit(percent);
                // a sensible starting value when the kind changes (Hwp: 160%, 15pt)
                spacing.set_value(if percent { 160.0 } else { 15.0 });
            });
        }
        let before = num(ps.spacing_before_pt, 0.0, 500.0);
        let after = num(ps.spacing_after_pt, 0.0, 500.0);
        let check = |t: &str, on: bool| {
            let c = gtk::CheckButton::with_label(t);
            c.set_active(on);
            c
        };
        let widow = check("외톨이줄 보호", ps.widow_orphan);
        let with_next = check("다음 문단과 함께", ps.keep_with_next);
        let keep = check("문단 보호", ps.keep_lines);
        let page_before = check("문단 앞에서 항상 쪽 나눔", ps.page_break_before);

        let with_unit = |w: &gtk::SpinButton, u: &str| {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            b.append(w);
            b.append(&gtk::Label::new(Some(u)));
            b
        };
        let cap = |t: &str| gtk::Label::builder().label(t).xalign(0.0).build();
        let head = |t: &str| {
            let l = gtk::Label::builder().label(t).xalign(0.0).build();
            l.add_css_class("heading");
            l
        };
        let grid = gtk::Grid::builder()
            .row_spacing(10)
            .column_spacing(12)
            .build();
        grid.attach(&head("정렬 방식"), 0, 0, 2, 1);
        let abox = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        abox.add_css_class("linked");
        for b in &align {
            abox.append(b);
        }
        grid.attach(&abox, 0, 1, 2, 1);
        grid.attach(&head("여백"), 0, 2, 2, 1);
        grid.attach(&cap("왼쪽"), 0, 3, 1, 1);
        grid.attach(&with_unit(&left, "pt"), 1, 3, 1, 1);
        grid.attach(&cap("오른쪽"), 0, 4, 1, 1);
        grid.attach(&with_unit(&right, "pt"), 1, 4, 1, 1);
        grid.attach(&head("첫 줄"), 0, 5, 2, 1);
        let first = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        first.append(&normal);
        first.append(&indent);
        first.append(&hanging);
        first.append(&with_unit(&first_amount, "pt"));
        grid.attach(&first, 0, 6, 2, 1);
        grid.attach(&head("간격"), 0, 7, 2, 1);
        grid.attach(&cap("줄 간격"), 0, 8, 1, 1);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        line.append(&kind);
        line.append(&spacing);
        line.append(&unit);
        grid.attach(&line, 1, 8, 1, 1);
        grid.attach(&cap("문단 위"), 0, 9, 1, 1);
        grid.attach(&with_unit(&before, "pt"), 1, 9, 1, 1);
        grid.attach(&cap("문단 아래"), 0, 10, 1, 1);
        grid.attach(&with_unit(&after, "pt"), 1, 10, 1, 1);
        grid.attach(&head("줄 나눔"), 0, 11, 2, 1);
        let breaks = gtk::Grid::builder()
            .column_spacing(12)
            .row_spacing(4)
            .build();
        breaks.attach(&widow, 0, 0, 1, 1);
        breaks.attach(&with_next, 1, 0, 1, 1);
        breaks.attach(&keep, 0, 1, 1, 1);
        breaks.attach(&page_before, 1, 1, 1, 1);
        grid.attach(&breaks, 0, 12, 2, 1);

        let d = adw::AlertDialog::new(Some("문단 모양"), None);
        d.set_extra_child(Some(&grid));
        d.add_responses(&[("cancel", "취소"), ("apply", "설정")]);
        d.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("apply"));
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            let Some(ed) = weak.upgrade() else { return };
            if r != "apply" {
                return;
            }
            let ind = if normal.is_active() {
                0.0
            } else if indent.is_active() {
                first_amount.value()
            } else {
                -first_amount.value()
            };
            let near = |a: f64, b: f64| (a - b).abs() < 0.05;
            let line = (KINDS[kind.selected() as usize].0, spacing.value());
            let changed = |v: bool, was: bool| (v != was).then_some(v);
            let f = ParaFormat {
                line_spacing: (line.0 != ps.line_spacing.0 || !near(line.1, ps.line_spacing.1))
                    .then_some(line),
                margin_left_pt: (!near(left.value(), ps.margin_left_pt)).then(|| left.value()),
                margin_right_pt: (!near(right.value(), ps.margin_right_pt)).then(|| right.value()),
                indent_pt: (!near(ind, ps.indent_pt)).then_some(ind),
                spacing_before_pt: (!near(before.value(), ps.spacing_before_pt))
                    .then(|| before.value()),
                spacing_after_pt: (!near(after.value(), ps.spacing_after_pt))
                    .then(|| after.value()),
                widow_orphan: changed(widow.is_active(), ps.widow_orphan),
                keep_with_next: changed(with_next.is_active(), ps.keep_with_next),
                keep_lines: changed(keep.is_active(), ps.keep_lines),
                page_break_before: changed(page_before.is_active(), ps.page_break_before),
                ..Default::default()
            };
            if f != ParaFormat::default() {
                ed.apply_para(f);
            }
            if let Some(i) = align.iter().position(|b| b.is_active()) {
                if ALIGNS[i] != ps.align {
                    ed.apply_align(ALIGNS[i]);
                }
            }
        });
        d.present(Some(&self.win));
    }

    /// Select (or clear) a table as an object and redraw its pages.
    fn select_table(&self, cell: Option<Pos>) {
        let old = std::mem::replace(&mut self.state.borrow_mut().table_selected, cell);
        if old != cell {
            for a in self.areas.borrow().iter() {
                a.queue_draw();
            }
        }
    }

    /// Copy the table at the caret (or the selected table) as an object.
    fn copy_table(&self) -> bool {
        let t = {
            let st = self.state.borrow();
            st.table_selected
                .or(st.caret)
                .filter(|p| matches!(p, Pos::Cell { .. }))
        };
        let Some(t) = t else { return false };
        let result = {
            let mut st = self.state.borrow_mut();
            let Some(l) = st.loaded.as_mut() else {
                return false;
            };
            l.doc
                .copy_table(t)
                .map(|_| l.doc.internal_clipboard_text().unwrap_or_default())
        };
        match result {
            Ok(text) => {
                // a marker text, so paste can tell our object copy from other clipboard text
                let marker = format!(
                    "{}\u{200b}",
                    if text.is_empty() {
                        "[표]".to_owned()
                    } else {
                        text
                    }
                );
                self.win.clipboard().set_text(&marker);
                self.state.borrow_mut().object_clip_text = Some(marker);
                self.toast("표를 복사했습니다");
                true
            }
            Err(e) => {
                self.toast(&format!("표를 복사할 수 없습니다: {e}"));
                false
            }
        }
    }

    /// Right-click: a menu for what is under the pointer (text, a cell, a selected table).
    fn context_menu(self: &Rc<Self>, w: &gtk::Widget, page: u32, x: f64, y: f64) {
        // move the caret to the click unless it lands inside the selection or selected table
        let zoom = self.state.borrow().zoom;
        let inside_sel = self.state.borrow().sel_rects.iter().any(|r| {
            r.page == page
                && x / zoom >= r.x
                && x / zoom <= r.x + r.w
                && y / zoom >= r.y
                && y / zoom <= r.y + r.h
        });
        let on_table = self.state.borrow().table_selected.is_some_and(|t| {
            let st = self.state.borrow();
            st.loaded.as_ref().is_some_and(|l| {
                l.doc.cell_boxes(t, page).iter().any(|b| {
                    x / zoom >= b.x
                        && x / zoom <= b.x + b.w
                        && y / zoom >= b.y
                        && y / zoom <= b.y + b.h
                })
            })
        });
        if !inside_sel && !on_table {
            self.on_press(page, x, y, false);
        }
        let (in_cell, object) = {
            let st = self.state.borrow();
            (
                matches!(st.caret, Some(Pos::Cell { .. })),
                st.table_selected.is_some(),
            )
        };
        let menu = gio::Menu::new();
        let clip = gio::Menu::new();
        clip.append(Some("오려 두기"), Some("win.cut"));
        clip.append(Some("복사하기"), Some("win.copy"));
        clip.append(Some("붙이기"), Some("win.paste"));
        menu.append_section(None, &clip);
        if object {
            let t = gio::Menu::new();
            t.append(Some("표 복사하기"), Some("win.table-copy"));
            t.append(Some("표 지우기"), Some("win.table-delete"));
            menu.append_section(None, &t);
        } else {
            let fmt = gio::Menu::new();
            fmt.append(Some("글자 모양…"), Some("win.char-shape"));
            fmt.append(Some("문단 모양…"), Some("win.para-shape"));
            menu.append_section(None, &fmt);
            if in_cell {
                let rows = gio::Menu::new();
                rows.append(Some("줄/칸 추가하기…"), Some("win.row-col-insert"));
                rows.append(Some("줄/칸 지우기…"), Some("win.row-col-delete"));
                menu.append_section(None, &rows);
                let eq = gio::Menu::new();
                eq.append(Some("셀 높이를 같게"), Some("win.table-row-equal"));
                eq.append(Some("셀 너비를 같게"), Some("win.table-col-equal"));
                menu.append_section(None, &eq);
                let cells = gio::Menu::new();
                cells.append(Some("표/셀 속성…"), Some("win.table-props"));
                cells.append(Some("셀 합치기"), Some("win.cell-merge"));
                cells.append(Some("셀 나누기"), Some("win.cell-split"));
                menu.append_section(None, &cells);
                let t = gio::Menu::new();
                t.append(Some("표 선택 (Esc)"), Some("win.table-select"));
                t.append(Some("표 복사하기"), Some("win.table-copy"));
                t.append(Some("표 지우기"), Some("win.table-delete"));
                menu.append_section(None, &t);
            } else {
                let ins = gio::Menu::new();
                ins.append(Some("표 만들기"), Some("win.insert-table"));
                ins.append(Some("쪽 나누기"), Some("win.page-break"));
                menu.append_section(None, &ins);
            }
        }
        let pop = gtk::PopoverMenu::from_model(Some(&menu));
        pop.set_parent(w);
        pop.set_has_arrow(false);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            // unparent after the closing animation and any activated action
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    /// Alt+Enter / Alt+Delete: Hwp's 줄/칸 추가하기·지우기 as a quick choice.
    fn row_col_dialog(self: &Rc<Self>, insert: bool) {
        if !self.in_table() {
            return;
        }
        let d = adw::AlertDialog::new(
            Some(if insert {
                "줄/칸 추가하기"
            } else {
                "줄/칸 지우기"
            }),
            None,
        );
        if insert {
            // stacked responses are listed last-first: this shows 위쪽…오른쪽, then 취소
            d.add_responses(&[
                ("cancel", "취소"),
                ("right", "오른쪽에 칸"),
                ("left", "왼쪽에 칸"),
                ("below", "아래쪽에 줄"),
                ("above", "위쪽에 줄"),
            ]);
            d.set_default_response(Some("below"));
        } else {
            d.add_responses(&[
                ("cancel", "취소"),
                ("table", "표 지우기"),
                ("col", "칸 지우기"),
                ("row", "줄 지우기"),
            ]);
            d.set_response_appearance("table", adw::ResponseAppearance::Destructive);
            d.set_default_response(Some("row"));
        }
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            let Some(ed) = weak.upgrade() else { return };
            let action = match r {
                "above" => "table-row-above",
                "below" => "table-row-below",
                "left" => "table-col-left",
                "right" => "table-col-right",
                "row" => "table-row-delete",
                "col" => "table-col-delete",
                "table" => "table-delete",
                _ => return,
            };
            WidgetExt::activate_action(&ed.win, &format!("win.{action}"), None).ok();
        });
        d.present(Some(&self.win));
    }

    /// One point bigger or smaller (selection or next typed text): Ctrl+] / Ctrl+[ (작게 also Alt+Shift+R).
    fn font_size_step(self: &Rc<Self>, delta: f64) {
        let size = {
            let st = self.state.borrow();
            let (Some(l), Some(p)) = (&st.loaded, st.caret) else {
                return;
            };
            let at = self
                .selection()
                .map_or(p, |r| r.start.with_offset(r.start.offset() + 1));
            l.doc
                .char_state(at)
                .map(|c| {
                    st.pending
                        .as_ref()
                        .and_then(|f| f.size_pt)
                        .unwrap_or(c.size_pt)
                })
                .ok()
        };
        if let Some(s) = size {
            self.apply_format(CharFormat {
                size_pt: Some((s + delta).clamp(1.0, 300.0)),
                ..Default::default()
            });
        }
    }

    /// Alt+Shift+Z / A: line spacing 10% up or down.
    fn line_spacing_step(self: &Rc<Self>, delta: i32) {
        let cur = {
            let st = self.state.borrow();
            let (Some(l), Some(p)) = (&st.loaded, st.caret) else {
                return;
            };
            l.doc.para_state(p).and_then(|s| s.line_spacing_pct)
        };
        match cur {
            Some(c) => self.apply_para(ParaFormat {
                line_spacing_pct: Some((c.round() as i32 + delta).clamp(50, 500)),
                ..Default::default()
            }),
            None => self.toast("고정·최소 줄 간격은 문단 모양에서 바꾸세요"),
        }
    }

    fn in_table(&self) -> bool {
        matches!(self.state.borrow().caret, Some(Pos::Cell { .. }))
    }

    fn resize_key(self: &Rc<Self>, key: gdk::Key, mode: Resize) {
        let (vertical, steps) = match key {
            gdk::Key::Left => (false, -1),
            gdk::Key::Right => (false, 1),
            gdk::Key::Up => (true, -1),
            _ => (true, 1),
        };
        self.edit(move |d, p, _| d.resize_table(p, vertical, steps, mode).map(Some));
    }

    fn page_break(self: &Rc<Self>) {
        self.edit(|d, p, sel| {
            let p = match sel {
                Some(r) => d.delete_range(r)?,
                None => p,
            };
            d.insert_page_break(p).map(Some)
        });
    }

    /// Fit the first page's width (or the whole page) into the view.
    fn zoom_fit(&self, page: bool) {
        let size = self
            .state
            .borrow()
            .loaded
            .as_ref()
            .and_then(|l| l.doc.page_size(0).ok());
        let Some(sz) = size else { return };
        let (w, h) = (
            self.scroller.width() as f64 - 2.0 * PAGE_GAP as f64,
            self.scroller.height() as f64 - 2.0 * PAGE_GAP as f64,
        );
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let mut z = w / sz.width;
        if page {
            z = z.min(h / sz.height);
        }
        self.set_zoom(z.clamp(0.1, 5.0));
    }

    fn default_name(&self, ext: &str) -> (String, Option<PathBuf>) {
        let st = self.state.borrow();
        let f = st.loaded.as_ref().and_then(|l| l.file.as_ref());
        let stem = f
            .and_then(|f| f.path().file_stem())
            .map_or("새 문서".into(), |s| s.to_string_lossy().into_owned());
        (
            format!("{stem}.{ext}"),
            f.and_then(|f| f.path().parent()).map(Path::to_path_buf),
        )
    }

    fn export_pdf(self: &Rc<Self>) {
        let (name, dir) = self.default_name("pdf");
        let dialog = gtk::FileDialog::builder()
            .title("PDF로 내보내기")
            .initial_name(name)
            .modal(true)
            .build();
        if let Some(d) = dir {
            dialog.set_initial_folder(Some(&gio::File::for_path(d)));
        }
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.win), gio::Cancellable::NONE, move |res| {
            let (Some(ed), Ok(file)) = (weak.upgrade(), res) else {
                return;
            };
            let Some(path) = file.path() else { return };
            let pdf = ed
                .state
                .borrow()
                .loaded
                .as_ref()
                .map(|l| l.doc.export_pdf());
            match pdf {
                Some(Ok(bytes)) => {
                    let tmp = path.with_extension("pdf.part");
                    match std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &path)) {
                        Ok(()) => ed.toast(&format!("PDF로 내보냈습니다: {}", path.display())),
                        Err(e) => {
                            let _ = std::fs::remove_file(&tmp);
                            ed.alert("PDF를 쓸 수 없습니다", &e.to_string());
                        }
                    }
                }
                Some(Err(e)) => ed.alert("PDF로 내보낼 수 없습니다", &e.to_string()),
                None => {}
            }
        });
    }

    /// Print through GTK: each page is rasterized at the printer's resolution.
    fn print(self: &Rc<Self>) {
        let pages = self
            .state
            .borrow()
            .loaded
            .as_ref()
            .map_or(0, |l| l.doc.page_count());
        if pages == 0 {
            return;
        }
        let op = gtk::PrintOperation::new();
        op.set_n_pages(pages as i32);
        op.set_use_full_page(true);
        op.set_job_name(&self.title.title());
        let weak = Rc::downgrade(self);
        op.connect_draw_page(move |_, ctx, page| {
            let Some(ed) = weak.upgrade() else { return };
            let st = ed.state.borrow();
            let Some(l) = &st.loaded else { return };
            let (Ok(svg), Ok(sz)) = (
                l.doc.render_page_svg(page as u32),
                l.doc.page_size(page as u32),
            ) else {
                return;
            };
            let scale = (ctx.width() / sz.width).min(ctx.height() / sz.height);
            if let Ok(surf) = render::rasterize(&svg, scale) {
                let cr = ctx.cairo_context();
                let _ = cr.set_source_surface(&surf, 0.0, 0.0);
                let _ = cr.paint();
            }
        });
        if let Err(e) = op.run(gtk::PrintOperationAction::PrintDialog, Some(&self.win)) {
            self.alert("인쇄할 수 없습니다", &e.to_string());
        }
    }

    /// Ask once per document before letting rhwp rewrite an edited HWPX.
    fn confirm_hwpx_rewrite(self: &Rc<Self>, then: impl FnOnce(&Rc<Editor>) + 'static) {
        let needs = {
            let st = self.state.borrow();
            matches!(&st.loaded, Some(l) if l.doc.format() == Format::Hwpx && l.doc.edited())
                && !st.hwpx_rewrite_ok
        };
        if !needs {
            then(self);
            return;
        }
        let d = adw::AlertDialog::new(
            Some("HWPX 저장 확인"),
            Some(
                "편집한 HWPX를 저장하면 엔진(rhwp)이 문서 XML을 다시 씁니다.\n\
                 검증 결과 본문 내용은 유지되지만 문단 ID 재번호, 일부 기본값 속성 생략, \
                 ZIP 항목 순서 변경이 확인되었습니다. 한컴 프로그램에서의 표시는 아직 검증되지 않았습니다.\n\n\
                 저장 전에 원본은 백업 폴더에 복사됩니다.",
            ),
        );
        d.add_responses(&[("cancel", "취소"), ("save", "저장")]);
        d.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        let then = RefCell::new(Some(then));
        d.connect_response(None, move |_, r| {
            let (Some(ed), Some(then)) = (weak.upgrade(), then.borrow_mut().take()) else {
                return;
            };
            if r == "save" {
                ed.state.borrow_mut().hwpx_rewrite_ok = true;
                then(&ed);
            }
        });
        d.present(Some(&self.win));
    }

    /// Save to the current path; `done(true)` after a successful save.
    pub fn save_in_place(self: &Rc<Self>, done: impl FnOnce(bool) + 'static) {
        let path = {
            let st = self.state.borrow();
            let Some(l) = &st.loaded else {
                drop(st);
                done(false);
                return;
            };
            l.file.as_ref().map(|f| f.path().to_path_buf())
        };
        match path {
            Some(path) => self.confirm_hwpx_rewrite(move |ed| done(ed.save_to(&path))),
            // a new document has nowhere to go yet
            None => self.save_as_then(done),
        }
    }

    fn save_as(self: &Rc<Self>) {
        self.save_as_then(|_| {});
    }

    /// "Save as". An opened file keeps its format; a new document takes the format
    /// of the chosen extension (.hwpx, otherwise .hwp).
    fn save_as_then(self: &Rc<Self>, done: impl FnOnce(bool) + 'static) {
        let (fixed, ext) = {
            let st = self.state.borrow();
            let Some(l) = &st.loaded else { return };
            (l.file.is_some(), l.doc.format().extension())
        };
        let (name, dir) = self.default_name(ext);
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(if fixed {
            "같은 형식"
        } else {
            "한글 문서 (*.hwp, *.hwpx)"
        }));
        filter.add_suffix(ext);
        if !fixed {
            filter.add_suffix("hwpx");
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(if fixed {
                "다른 이름으로 저장"
            } else {
                "새 문서 저장 (.hwp 또는 .hwpx)"
            })
            .initial_name(name)
            .filters(&filters)
            .modal(true)
            .build();
        if let Some(d) = dir {
            dialog.set_initial_folder(Some(&gio::File::for_path(d)));
        }
        let weak = Rc::downgrade(self);
        let done = RefCell::new(Some(done));
        dialog.save(Some(&self.win), gio::Cancellable::NONE, move |res| {
            let Some(done) = done.borrow_mut().take() else {
                return;
            };
            let (Some(ed), Ok(file)) = (weak.upgrade(), res) else {
                done(false);
                return;
            };
            let Some(mut path) = file.path() else {
                done(false);
                return;
            };
            let chosen = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            let fmt = if fixed {
                ed.state
                    .borrow()
                    .loaded
                    .as_ref()
                    .map_or(Format::Hwp, |l| l.doc.format())
            } else if chosen.as_deref() == Some("hwpx") {
                Format::Hwpx
            } else {
                Format::Hwp
            };
            if chosen.as_deref() != Some(fmt.extension()) {
                path = PathBuf::from(format!("{}.{}", path.display(), fmt.extension()));
            }
            if !fixed {
                if let Some(l) = ed.state.borrow_mut().loaded.as_mut() {
                    l.doc.set_format(fmt);
                }
            }
            ed.confirm_hwpx_rewrite(move |ed| done(ed.save_to(&path)));
        });
    }

    /// Save, but first compare the new bytes with the file on disk: if anything
    /// the editor cannot remove on purpose would disappear (e.g. HWP memos, which
    /// rhwp does not model), ask before writing. Cancel is the default.
    fn save_to(self: &Rc<Self>, target: &Path) -> bool {
        let check = {
            let st = self.state.borrow();
            let Some(l) = &st.loaded else { return false };
            match (&l.file, l.doc.edited()) {
                (Some(f), true) => l.doc.serialize(l.doc.format()).ok().map(|s| {
                    (
                        hwpedit_engine::loss::detect_losses_allowing(
                            f.bytes(),
                            &s.0,
                            l.doc.format(),
                            l.doc.removed_objects(),
                        ),
                        s.0,
                    )
                }),
                _ => None,
            }
        };
        match check {
            Some((Ok(losses), bytes)) if !losses.is_empty() => {
                let list: Vec<String> = losses.iter().map(|l| format!("• {l}")).collect();
                let d = adw::AlertDialog::new(
                    Some("저장하면 일부 내용이 사라집니다"),
                    Some(&format!(
                        "편집기가 다루지 못하는 요소가 저장 결과에서 빠집니다:\n\n{}\n\n\
                         직접 지운 것이 아니라면 저장하지 마세요. 「다른 이름으로 저장」해도 같은 내용이 빠집니다.",
                        list.join("\n")
                    )),
                );
                d.add_responses(&[("cancel", "저장하지 않음"), ("save", "그래도 저장")]);
                d.set_response_appearance("save", adw::ResponseAppearance::Destructive);
                d.set_default_response(Some("cancel"));
                d.set_close_response("cancel");
                let weak = Rc::downgrade(self);
                let target = target.to_path_buf();
                let bytes = RefCell::new(Some(bytes));
                d.connect_response(None, move |_, r| {
                    if let (Some(ed), "save", Some(b)) =
                        (weak.upgrade(), r, bytes.borrow_mut().take())
                    {
                        ed.write_to(&target, Some(b));
                    }
                });
                d.present(Some(&self.win));
                false
            }
            Some((Err(e), _)) => {
                self.alert("저장하지 않았습니다", &format!("저장 결과의 손실 여부를 검사하지 못했습니다: {e}\n\n원본 파일은 바뀌지 않았습니다."));
                false
            }
            Some((Ok(_), bytes)) => self.write_to(target, Some(bytes)),
            None => self.write_to(target, None),
        }
    }

    /// `pre`: bytes already serialized (and checked) for this save.
    fn write_to(&self, target: &Path, pre: Option<Vec<u8>>) -> bool {
        let opts = SaveOptions::default();
        let result = {
            let mut st = self.state.borrow_mut();
            let Some(l) = st.loaded.as_mut() else {
                return false;
            };
            let fmt = l.doc.format();
            let edited = l.doc.edited();
            let doc = &l.doc;
            let r = match l.file.as_mut() {
                Some(file) => save(
                    file,
                    target,
                    fmt,
                    edited,
                    |f| match pre {
                        Some(b) => Ok(b),
                        None => doc.serialize(f).map(|s| s.0).map_err(|e| e.to_string()),
                    },
                    Document::validate,
                    &opts,
                ),
                None => doc
                    .serialize(fmt)
                    .map_err(|e| SaveError::Serialize(e.to_string()))
                    .and_then(|s| save_new(target, fmt, s.0, Document::validate, &opts))
                    .map(|(file, backup)| {
                        l.file = Some(file);
                        SaveOutcome::Written { backup }
                    }),
            };
            if r.is_ok() {
                l.doc.mark_saved();
            }
            r
        };
        let ok = match result {
            Ok(SaveOutcome::Unchanged) => {
                self.toast("변경 사항이 없어 파일을 다시 쓰지 않았습니다");
                true
            }
            Ok(SaveOutcome::CopiedOriginal { backup }) => {
                self.toast(&saved_msg(
                    "원본 그대로 복사해 저장했습니다",
                    backup.as_deref(),
                ));
                true
            }
            Ok(SaveOutcome::Written { backup }) => {
                self.toast(&saved_msg("저장했습니다", backup.as_deref()));
                add_recent(target);
                true
            }
            Err(e @ SaveError::ChangedOnDisk(_)) => {
                self.alert(
                    "저장하지 않았습니다",
                    &format!("{e}\n\n다른 이름으로 저장해 두 버전을 모두 보존하세요."),
                );
                false
            }
            Err(e) => {
                self.alert(
                    "저장하지 않았습니다",
                    &format!("{e}\n\n원본 파일은 바뀌지 않았습니다."),
                );
                false
            }
        };
        self.refresh_chrome();
        ok
    }

    fn install_close_guard(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.win.connect_close_request(move |win| {
            let Some(ed) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let edited = ed
                .state
                .borrow()
                .loaded
                .as_ref()
                .is_some_and(|l| l.doc.edited());
            if !edited {
                return glib::Propagation::Proceed;
            }
            let d = adw::AlertDialog::new(
                Some("저장하지 않은 변경 사항"),
                Some("닫기 전에 변경 사항을 저장할까요?"),
            );
            d.add_responses(&[
                ("cancel", "취소"),
                ("discard", "저장 안 함"),
                ("save", "저장"),
            ]);
            d.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
            d.set_response_appearance("save", adw::ResponseAppearance::Suggested);
            d.set_default_response(Some("save"));
            d.set_close_response("cancel");
            let weak = Rc::downgrade(&ed);
            let win = win.clone();
            d.connect_response(None, move |_, r| {
                let Some(ed) = weak.upgrade() else { return };
                match r {
                    "discard" => {
                        if let Some(l) = ed.state.borrow_mut().loaded.as_mut() {
                            l.doc.mark_saved();
                        }
                        win.close();
                    }
                    "save" => {
                        let win = win.clone();
                        ed.save_in_place(move |ok| {
                            if ok {
                                win.close();
                            }
                        });
                    }
                    _ => {}
                }
            });
            d.present(Some(&ed.win));
            glib::Propagation::Stop
        });
    }
}

/// Highlight for a block of cells: the box spanning both corner cells (same page only).
fn cell_block_rect(doc: &Document, a: Pos, b: Pos) -> Option<SelRect> {
    let (ra, rb) = (doc.caret_rect(a).ok()?, doc.caret_rect(b).ok()?);
    let ((ax, ay, aw, ah), (bx, by, bw, bh)) = (ra.cell?, rb.cell?);
    if ra.page != rb.page {
        return None;
    }
    let (x0, y0) = (ax.min(bx), ay.min(by));
    let (x1, y1) = ((ax + aw).max(bx + bw), (ay + ah).max(by + bh));
    Some(SelRect {
        page: ra.page,
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

fn add_recent(path: &Path) {
    let uri = gio::File::for_path(path).uri();
    gtk::RecentManager::default().add_item(&uri);
}

/// Recently used HWP/HWPX files that still exist, newest first.
pub fn recent_documents(limit: usize) -> Vec<PathBuf> {
    let mut items: Vec<_> = gtk::RecentManager::default()
        .items()
        .into_iter()
        .filter_map(|i| {
            let path = gio::File::for_uri(&i.uri()).path()?;
            let ext = path.extension()?.to_str()?.to_ascii_lowercase();
            (matches!(ext.as_str(), "hwp" | "hwpx") && path.exists())
                .then(|| (i.modified().to_unix(), path))
        })
        .collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.0));
    items.into_iter().take(limit).map(|(_, p)| p).collect()
}

/// Later fields win.
fn merge(mut a: CharFormat, b: CharFormat) -> CharFormat {
    a.bold = b.bold.or(a.bold);
    a.italic = b.italic.or(a.italic);
    a.underline = b.underline.or(a.underline);
    a.strike = b.strike.or(a.strike);
    a.size_pt = b.size_pt.or(a.size_pt);
    a.color = b.color.or(a.color);
    a.font = b.font.or(a.font);
    a
}

/// Show pending (not yet typed) formatting in the ribbon.
fn overlay(c: &mut hwpedit_engine::CharState, f: &CharFormat) {
    if let Some(v) = f.bold {
        c.bold = v;
    }
    if let Some(v) = f.italic {
        c.italic = v;
    }
    if let Some(v) = f.underline {
        c.underline = v;
    }
    if let Some(v) = f.strike {
        c.strike = v;
    }
    if let Some(v) = f.size_pt {
        c.size_pt = v;
    }
    if let Some(v) = &f.color {
        c.color = v.clone();
    }
    if let Some(v) = &f.font {
        c.font = v.clone();
    }
}

/// `a` comes before `b` in a container (same body section or same cell).
fn precedes(a: Pos, b: Pos) -> bool {
    Range::new(a, b).is_some_and(|r| r.start == a && a != b)
        || (Range::new(a, b).is_none() && section_of(a) < section_of(b))
}

fn section_of(p: Pos) -> usize {
    match p {
        Pos::Body { section, .. } | Pos::Cell { section, .. } => section,
    }
}

fn saved_msg(what: &str, backup: Option<&Path>) -> String {
    match backup {
        Some(b) => format!("{what} · 이전 파일 백업: {}", b.display()),
        None => what.to_owned(),
    }
}

/// Installed font families, sorted, for the font list.
fn font_families(w: &impl IsA<gtk::Widget>) -> Vec<String> {
    let mut v: Vec<String> = w
        .pango_context()
        .list_families()
        .iter()
        .map(|f| f.name().to_string())
        .collect();
    v.sort();
    v.dedup();
    v
}

fn para_index(p: Pos) -> usize {
    match p {
        Pos::Body { para, .. } => para,
        Pos::Cell { cell_para, .. } => cell_para,
    }
}

fn prev_para(p: Pos) -> Option<Pos> {
    match p {
        Pos::Body { section, para, .. } if para > 0 => Some(Pos::Body {
            section,
            para: para - 1,
            offset: 0,
        }),
        Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            cell_para,
            ..
        } if cell_para > 0 => Some(Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            cell_para: cell_para - 1,
            offset: 0,
        }),
        _ => None,
    }
}

fn next_para(p: Pos) -> Option<Pos> {
    match p {
        Pos::Body { section, para, .. } => Some(Pos::Body {
            section,
            para: para + 1,
            offset: 0,
        }),
        Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            cell_para,
            ..
        } => Some(Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            cell_para: cell_para + 1,
            offset: 0,
        }),
    }
}
