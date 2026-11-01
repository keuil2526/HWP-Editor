//! Hancom-style command area on liquid glass:
//! - a menu row like Hwp 2022's: 파일 (a drop-down menu), then the 편집 · 보기 · 입력 · 서식
//!   tabs (plus a contextual 표 레이아웃 tab inside tables) and a 「찾을 내용」 box,
//!   over a ribbon of large labelled buttons, and
//! - the always-visible 서식 도구 상자 (font, size, B/I/U/S, colour, alignment, line spacing).
//!
//! Buttons that map 1:1 to window actions use `action-name`; stateful controls are
//! wired by the window, which also pushes the caret's formatting back into them.

use crate::glass::GlassPane;
use crate::icons;
use gtk::gdk;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

pub struct Ribbon {
    /// Tabs + ribbon panel.
    pub ribbon_pane: GlassPane,
    /// 서식 도구 상자.
    pub toolbar_pane: GlassPane,
    tabs: Vec<(String, gtk::ToggleButton)>,
    /// 표 디자인, 표 레이아웃: shown only with the caret in a table.
    table_tabs: [gtk::ToggleButton; 2],
    pub bold: gtk::ToggleButton,
    pub italic: gtk::ToggleButton,
    pub underline: gtk::ToggleButton,
    pub strike: gtk::ToggleButton,
    pub font: gtk::DropDown,
    pub fonts: gtk::StringList,
    /// 스타일 in the 서식 도구 상자: the document's styles, in stored order (index = id).
    pub style: gtk::DropDown,
    styles: gtk::StringList,
    pub size: gtk::SpinButton,
    pub color: gtk::ColorDialogButton,
    /// justify, left, center, right, distribute, split (Hwp's toolbar order)
    pub align: [gtk::ToggleButton; 6],
    pub line_spacing: gtk::SpinButton,
    /// 「찾을 내용」 in the menu row.
    pub find_entry: gtk::SearchEntry,
    pub table_rows: gtk::SpinButton,
    pub table_cols: gtk::SpinButton,
    pub zoom_label: gtk::Label,
}

/// Tooltip for a control Hwp 2022 has that we do not implement yet.
const NOT_YET: &str = "아직 지원하지 않음";

/// Large ribbon button: icon over label, like Hancom's ribbon.
/// `label` may hold a line break, as Hwp writes 「오려 / 두기」.
fn big(icon: &str, label: &str, tip: &str, action: &str) -> gtk::Button {
    let b = gtk::Button::builder()
        .child(&big_content(icon, label, false))
        .tooltip_text(tip)
        .action_name(action)
        .build();
    b.add_css_class("big");
    b.set_focus_on_click(false);
    b
}

/// Mark `b` as a Hwp 2022 control we do not support yet: drawn grey (not insensitive,
/// which the theme fades to near-invisible) and clicking it says so (`win.not-yet`).
fn mark_pending(b: &gtk::Button, name: &str) {
    b.add_css_class("pending");
    b.set_tooltip_text(Some(&format!("{name} — {NOT_YET}")));
    b.set_action_target_value(Some(&name.to_variant()));
    b.set_action_name(Some("win.not-yet"));
    b.set_focus_on_click(false);
}

/// A Hwp 2022 ribbon button we do not support yet, shown in its place.
/// `drop` draws the ˅ that Hwp shows for buttons with a drop-down.
fn pending(icon: &str, label: &str, drop: bool) -> gtk::Button {
    let b = gtk::Button::builder()
        .child(&big_content(icon, label, drop))
        .build();
    b.add_css_class("big");
    mark_pending(&b, &label.replace('\n', " "));
    b
}

fn drop_mark() -> gtk::Image {
    let i = gtk::Image::from_icon_name("pan-down-symbolic");
    i.set_pixel_size(10);
    i.add_css_class("drop-mark");
    i
}

fn big_content(icon: &str, label: &str, drop: bool) -> gtk::Box {
    let v = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let img = gtk::Image::from_icon_name(&icons::name(icon));
    img.set_pixel_size(22);
    img.set_margin_bottom(2);
    v.append(&img);
    let lines: Vec<&str> = label.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let l = gtk::Label::new(Some(line));
        if i + 1 < lines.len() || !drop {
            v.append(&l);
        } else if lines.len() == 1 {
            // one-line label: the ˅ goes on its own line, as in Hwp
            v.append(&l);
            v.append(&drop_mark());
        } else {
            let h = gtk::Box::new(gtk::Orientation::Horizontal, 1);
            h.set_halign(gtk::Align::Center);
            h.append(&l);
            h.append(&drop_mark());
            v.append(&h);
        }
    }
    v
}

/// Small icon button for the toolbar.
fn small(icon: &str, tip: &str, action: &str) -> gtk::Button {
    let b = gtk::Button::builder()
        .icon_name(icons::name(icon))
        .tooltip_text(tip)
        .action_name(action)
        .build();
    b.set_focus_on_click(false);
    b
}

/// Small button of Hwp's 표 디자인 tab, not supported yet: icon, then `label` and ˅
/// when `with_text` (otherwise `label` is only the tooltip).
fn pending_small(icon: &str, label: &str, with_text: bool) -> gtk::Button {
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    h.append(&gtk::Image::from_icon_name(&icons::name(icon)));
    if with_text {
        h.append(&gtk::Label::new(Some(label)));
        h.append(&drop_mark());
    }
    let b = gtk::Button::builder().child(&h).build();
    b.add_css_class("flat-small");
    mark_pending(&b, label);
    b
}

/// Small button with icon, label and ˅ that opens `menu` (서식 › 글머리표˅).
fn small_menu(icon: &str, label: &str, tip: &str, menu: &gtk::gio::Menu) -> gtk::MenuButton {
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    h.append(&gtk::Image::from_icon_name(&icons::name(icon)));
    h.append(&gtk::Label::new(Some(label)));
    h.append(&drop_mark());
    let b = gtk::MenuButton::builder()
        .child(&h)
        .menu_model(menu)
        .tooltip_text(tip)
        .build();
    b.add_css_class("flat-small");
    b.set_focus_on_click(false);
    b
}

/// Colours offered by 셀 음영˅ and 테두리 색˅ (rows: greys, then pale to deep hues).
const PALETTE: [&str; 40] = [
    "#ffffff", "#d9d9d9", "#bfbfbf", "#a6a6a6", "#808080", "#595959", "#404040", "#262626",
    "#0d0d0d", "#000000", "#ffe5e5", "#fff2cc", "#ffffcc", "#e2f0d9", "#ddebf7", "#dae3f3",
    "#ede1f5", "#fce4ec", "#f2f2f2", "#e7e6e6", "#ff9999", "#ffd966", "#ffff66", "#a9d18e",
    "#9dc3e6", "#8faadc", "#c5a5e0", "#f4a6c0", "#c9c9c9", "#aeaaaa", "#ff0000", "#ffc000",
    "#ffff00", "#70ad47", "#0070c0", "#4472c4", "#7030a0", "#e91e63", "#7f7f7f", "#3a3838",
];

/// Small button with icon, label and ˅ that opens a colour palette; a colour activates
/// `action` with "#rrggbb" ("" for `none`, when given).
fn color_menu(
    icon: &str,
    label: &str,
    tip: &str,
    action: &'static str,
    none: Option<&str>,
) -> gtk::MenuButton {
    let grid = gtk::Grid::builder()
        .row_spacing(2)
        .column_spacing(2)
        .build();
    let pop = gtk::Popover::new();
    for (i, hex) in PALETTE.into_iter().enumerate() {
        let swatch = gtk::DrawingArea::builder()
            .content_width(16)
            .content_height(16)
            .build();
        let rgba = gdk::RGBA::parse(hex).unwrap_or(gdk::RGBA::BLACK);
        swatch.set_draw_func(move |_, cr, w, h| {
            cr.set_source_rgb(rgba.red() as f64, rgba.green() as f64, rgba.blue() as f64);
            cr.rectangle(0.0, 0.0, w as f64, h as f64);
            let _ = cr.fill_preserve();
            cr.set_source_rgba(0.5, 0.5, 0.5, 0.6);
            cr.set_line_width(1.0);
            let _ = cr.stroke();
        });
        let b = gtk::Button::builder()
            .child(&swatch)
            .tooltip_text(hex)
            .action_name(action)
            .build();
        b.set_action_target_value(Some(&hex.to_variant()));
        b.add_css_class("flat");
        b.add_css_class("swatch");
        let p = pop.clone();
        b.connect_clicked(move |_| p.popdown());
        grid.attach(&b, (i % 10) as i32, (i / 10) as i32, 1, 1);
    }
    let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
    if let Some(none) = none {
        let b = gtk::Button::builder()
            .label(none)
            .action_name(action)
            .build();
        b.set_action_target_value(Some(&"".to_variant()));
        b.add_css_class("flat");
        let p = pop.clone();
        b.connect_clicked(move |_| p.popdown());
        v.append(&b);
    }
    v.append(&grid);
    // 다른 색…: any colour, through the system colour chooser
    let other = gtk::Button::builder().label("다른 색…").build();
    other.add_css_class("flat");
    let p = pop.clone();
    other.connect_clicked(move |b| {
        p.popdown();
        let root = b.root().and_downcast::<gtk::Window>();
        let widget = b.clone();
        gtk::ColorDialog::builder()
            .with_alpha(false)
            .build()
            .choose_rgba(root.as_ref(), None, gtk::gio::Cancellable::NONE, move |r| {
                if let Ok(c) = r {
                    let hex = format!(
                        "#{:02x}{:02x}{:02x}",
                        (c.red() * 255.0).round() as u8,
                        (c.green() * 255.0).round() as u8,
                        (c.blue() * 255.0).round() as u8
                    );
                    let _ = widget.activate_action(action, Some(&hex.to_variant()));
                }
            });
    });
    v.append(&other);
    pop.set_child(Some(&v));
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    h.append(&gtk::Image::from_icon_name(&icons::name(icon)));
    h.append(&gtk::Label::new(Some(label)));
    h.append(&drop_mark());
    let b = gtk::MenuButton::builder()
        .child(&h)
        .popover(&pop)
        .tooltip_text(tip)
        .build();
    b.add_css_class("flat-small");
    b.set_focus_on_click(false);
    b
}

/// 테두리˅: Hwp 2022's grid of edge pictures (a 2×2 block, the drawn edges in blue).
fn border_menu() -> gtk::MenuButton {
    let grid = gtk::Grid::builder()
        .row_spacing(2)
        .column_spacing(2)
        .build();
    let pop = gtk::Popover::new();
    for (i, (tip, side)) in crate::window::BORDER_SIDES.into_iter().enumerate() {
        let area = gtk::DrawingArea::builder()
            .content_width(24)
            .content_height(24)
            .build();
        area.set_draw_func(move |_, cr, w, h| {
            let (w, h) = (w as f64, h as f64);
            let (x0, y0, x1, y1, xm, ym) = (
                2.5,
                2.5,
                w - 2.5,
                h - 2.5,
                (w / 2.0).floor() + 0.5,
                (h / 2.0).floor() + 0.5,
            );
            // left, right, top, bottom, inner vertical, inner horizontal
            let edges = [
                ((x0, y0), (x0, y1)),
                ((x1, y0), (x1, y1)),
                ((x0, y0), (x1, y0)),
                ((x0, y1), (x1, y1)),
                ((xm, y0), (xm, y1)),
                ((x0, ym), (x1, ym)),
            ];
            let on: [bool; 6] = match side {
                "outer" => [true, true, true, true, false, false],
                "all" => [true; 6],
                "inner" => [false, false, false, false, true, true],
                "top" => [false, false, true, false, false, false],
                "bottom" => [false, false, false, true, false, false],
                "left" => [true, false, false, false, false, false],
                "right" => [false, true, false, false, false, false],
                "inner-h" => [false, false, false, false, false, true],
                "inner-v" => [false, false, false, false, true, false],
                _ => [false; 6],
            };
            for ((a, b), lit) in edges.iter().zip(on) {
                if lit {
                    cr.set_source_rgb(0.15, 0.4, 0.75);
                    cr.set_line_width(2.0);
                    cr.set_dash(&[], 0.0);
                } else {
                    cr.set_source_rgba(0.5, 0.5, 0.5, 0.9);
                    cr.set_line_width(1.0);
                    cr.set_dash(&[1.0, 1.5], 0.0);
                }
                cr.move_to(a.0, a.1);
                cr.line_to(b.0, b.1);
                let _ = cr.stroke();
            }
        });
        let b = gtk::Button::builder()
            .child(&area)
            .tooltip_text(tip)
            .action_name("win.cell-border")
            .build();
        b.set_action_target_value(Some(&side.to_variant()));
        b.add_css_class("flat");
        let p = pop.clone();
        b.connect_clicked(move |_| p.popdown());
        grid.attach(&b, (i % 4) as i32, (i / 4) as i32, 1, 1);
    }
    pop.set_child(Some(&grid));
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    h.append(&gtk::Image::from_icon_name(&icons::name("border")));
    h.append(&gtk::Label::new(Some("테두리")));
    h.append(&drop_mark());
    let b = gtk::MenuButton::builder()
        .child(&h)
        .popover(&pop)
        .tooltip_text("테두리: 고른 셀에 지금 테두리 종류·굵기·색으로")
        .build();
    b.add_css_class("flat-small");
    b.set_focus_on_click(false);
    b
}

/// A drop-down in the toolbar that only shows Hwp's default for now.
fn pending_drop(text: &str, tip: &str, width: i32) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(&[text]);
    d.set_tooltip_text(Some(&format!("{tip} — {NOT_YET}")));
    d.set_size_request(width, -1);
    d.set_sensitive(false);
    d
}

/// One ribbon item: B = working big button (icon, label, tooltip, action),
/// P = Hwp 2022 button we do not support yet (icon, label, has ˅), S = separator.
enum It<'a> {
    B(&'a str, &'a str, &'a str, &'a str),
    P(&'a str, &'a str, bool),
    S,
}
use It::*;

fn fill(row: &gtk::Box, items: &[It]) {
    for it in items {
        match *it {
            B(icon, label, tip, action) => row.append(&big(icon, label, tip, action)),
            P(icon, label, drop) => row.append(&pending(icon, label, drop)),
            S => row.append(&sep()),
        }
    }
}

/// 보기 check boxes that work (stateful window actions, see window/view.rs).
const WIRED_CHECKS: [(&str, &str); 2] = [
    ("문단 부호", "win.view-paragraph-marks"),
    ("투명 선", "win.view-transparent-lines"),
];

/// Hwp's check boxes in a ribbon (three to a column); those not in `WIRED_CHECKS`
/// are shown with Hwp's defaults and not yet wired.
fn checks(items: &[(&str, bool)]) -> gtk::Grid {
    let g = gtk::Grid::builder()
        .column_spacing(10)
        .row_spacing(0)
        .valign(gtk::Align::Center)
        .build();
    for (i, (label, on)) in items.iter().enumerate() {
        let c = gtk::CheckButton::builder().label(*label).build();
        match WIRED_CHECKS.iter().find(|(l, _)| l == label) {
            Some((_, action)) => c.set_action_name(Some(action)),
            None => {
                c.set_active(*on);
                c.set_sensitive(false);
                c.set_tooltip_text(Some(&format!("{label} — {NOT_YET}")));
            }
        }
        g.attach(&c, (i / 3) as i32, (i % 3) as i32, 1, 1);
    }
    g
}

/// 도형˅: the shapes this version draws, each asked for its size (and text, for 글상자).
fn shape_button() -> gtk::MenuButton {
    let m = gtk::gio::Menu::new();
    for (label, kind) in [
        ("직사각형", "rectangle"),
        ("타원", "ellipse"),
        ("직선", "line"),
    ] {
        let item = gtk::gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(Some("win.shape"), Some(&kind.to_variant()));
        m.append_item(&item);
    }
    // 글상자: rhwp writes a new text box into HWP so that Hwp 2022 cannot open the file
    // (HWPX opens) — offered once that is fixed
    m.append(Some("글상자 (아직 지원하지 않음)"), Some("app.none"));
    let b = gtk::MenuButton::builder()
        .child(&big_content("shape", "도형", true))
        .menu_model(&m)
        .tooltip_text("도형: 글자처럼 취급해 캐럿 자리에 넣습니다")
        .build();
    b.add_css_class("big");
    b
}

/// 단˅: 1·2·3단 or 단 설정….
fn columns_button() -> gtk::MenuButton {
    let m = gtk::gio::Menu::new();
    for n in 1..=3 {
        let item = gtk::gio::MenuItem::new(Some(&format!("{n}단")), None);
        item.set_action_and_target_value(Some("win.columns"), Some(&n.to_variant()));
        m.append_item(&item);
    }
    m.append(Some("단 설정…"), Some("win.columns-dialog"));
    let b = gtk::MenuButton::builder()
        .child(&big_content("columns", "단", true))
        .menu_model(&m)
        .tooltip_text("단")
        .build();
    b.add_css_class("big");
    b
}

/// A column of small buttons.
fn column(items: &[gtk::Button]) -> gtk::Box {
    let v = gtk::Box::new(gtk::Orientation::Vertical, 0);
    v.set_valign(gtk::Align::Center);
    for b in items {
        v.append(b);
    }
    v
}

/// Small working button with icon and label (e.g. 보기 › 쪽 맞춤).
fn small_labeled(icon: &str, label: &str, tip: &str, action: &str) -> gtk::Button {
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    h.append(&gtk::Image::from_icon_name(&icons::name(icon)));
    h.append(&gtk::Label::new(Some(label)));
    let b = gtk::Button::builder()
        .child(&h)
        .tooltip_text(tip)
        .action_name(action)
        .build();
    b.add_css_class("flat-small");
    b.set_focus_on_click(false);
    b
}

/// A tab's ˅ menu: sections of (label, action); "app.none" items show disabled.
fn tab_menu(sections: &[&[(&str, &str)]]) -> gtk::gio::Menu {
    // shortcuts the editor handles itself (not app accels) are only shown, as Hwp does
    let shown_accel = |action: &str| match action {
        "win.cut" => Some("<Control>x"),
        "win.copy" => Some("<Control>c"),
        "win.paste" => Some("<Control>v"),
        "win.select-all" => Some("<Control>a"),
        "win.page-break" => Some("<Control>Return"),
        "win.outline" => Some("<Control>Insert"),
        "win.list-number" => Some("<Control><Shift>Insert"),
        "win.list-bullet" => Some("<Control><Shift>Delete"),
        "win.level-up" => Some("<Control>KP_Subtract"),
        "win.level-down" => Some("<Control>KP_Add"),
        "win.style-dialog" => Some("F6"),
        "win.equation" => Some("<Alt><Shift>e"),
        _ => None,
    };
    let menu = gtk::gio::Menu::new();
    for items in sections {
        let m = gtk::gio::Menu::new();
        for (label, action) in *items {
            let item = gtk::gio::MenuItem::new(Some(label), Some(action));
            if let Some(accel) = shown_accel(action) {
                item.set_attribute_value("accel", Some(&accel.to_variant()));
            }
            m.append_item(&item);
        }
        menu.append_section(None, &m);
    }
    menu
}

fn unit(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("caption-label");
    l
}

fn text_toggle(markup: &str, tip: &str) -> gtk::ToggleButton {
    let label = gtk::Label::new(None);
    label.set_markup(markup);
    let b = gtk::ToggleButton::builder()
        .child(&label)
        .tooltip_text(tip)
        .build();
    b.set_focus_on_click(false);
    b
}

fn icon_toggle(icon: &str, tip: &str) -> gtk::ToggleButton {
    let b = gtk::ToggleButton::builder()
        .icon_name(icons::name(icon))
        .tooltip_text(tip)
        .build();
    b.set_focus_on_click(false);
    b
}

fn spin(min: f64, max: f64, step: f64, digits: u32, tip: &str) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(min, max, step);
    s.set_digits(digits);
    s.set_tooltip_text(Some(tip));
    s.set_width_chars(5);
    s
}

/// Hwp/HOP-style size picker: hover over a grid to choose rows x columns, click to insert.
fn table_grid(rows: &gtk::SpinButton, cols: &gtk::SpinButton) -> gtk::Box {
    const R: i32 = 8;
    const C: i32 = 10;
    const CELL: f64 = 18.0;
    let hover = Rc::new(Cell::new((0i32, 0i32)));
    let label = gtk::Label::new(Some("표 크기를 고르세요"));
    label.add_css_class("caption-label");
    let area = gtk::DrawingArea::builder()
        .content_width(C * CELL as i32 + 1)
        .content_height(R * CELL as i32 + 1)
        .build();
    let h = hover.clone();
    area.set_draw_func(move |a, cr, _, _| {
        let (hr, hc) = h.get();
        let fg = a.color();
        for r in 0..R {
            for c in 0..C {
                let (x, y) = (c as f64 * CELL + 0.5, r as f64 * CELL + 0.5);
                cr.rectangle(x + 1.0, y + 1.0, CELL - 3.0, CELL - 3.0);
                if r < hr && c < hc {
                    cr.set_source_rgba(0.21, 0.52, 0.89, 0.55);
                    let _ = cr.fill_preserve();
                }
                cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.35);
                cr.set_line_width(1.0);
                let _ = cr.stroke();
            }
        }
    });
    let motion = gtk::EventControllerMotion::new();
    let (h, l, a) = (hover.clone(), label.clone(), area.clone());
    motion.connect_motion(move |_, x, y| {
        let cell = (
            ((y / CELL) as i32 + 1).clamp(1, R),
            ((x / CELL) as i32 + 1).clamp(1, C),
        );
        if h.get() != cell {
            h.set(cell);
            l.set_text(&format!("{} × {} 표", cell.0, cell.1));
            a.queue_draw();
        }
    });
    let (h, l, a) = (hover.clone(), label.clone(), area.clone());
    motion.connect_leave(move |_| {
        h.set((0, 0));
        l.set_text("표 크기를 고르세요");
        a.queue_draw();
    });
    area.add_controller(motion);
    let click = gtk::GestureClick::new();
    let (h, rows, cols) = (hover, rows.clone(), cols.clone());
    click.connect_released(move |g, _, _, _| {
        let (r, c) = h.get();
        if r == 0 {
            return;
        }
        rows.set_value(r as f64);
        cols.set_value(c as f64);
        if let Some(w) = g.widget() {
            if let Some(p) = w
                .ancestor(gtk::Popover::static_type())
                .and_downcast::<gtk::Popover>()
            {
                p.popdown();
            }
            let _ = w.activate_action("win.insert-table", None);
        }
    });
    area.add_controller(click);
    let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
    v.append(&area);
    v.append(&label);
    v
}

fn row() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    b.add_css_class("ribbon-panel");
    b
}

fn sep() -> gtk::Separator {
    let s = gtk::Separator::new(gtk::Orientation::Vertical);
    s.set_margin_start(4);
    s.set_margin_end(4);
    s
}

fn linked(children: &[&gtk::Widget]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("linked");
    for c in children {
        b.append(*c);
    }
    b
}

impl Ribbon {
    pub fn new(font_names: &[String], backdrop: &impl IsA<gtk::Widget>) -> Ribbon {
        // ---- 파일: a menu, as in Hwp 2022
        let file_menu = gtk::gio::Menu::new();
        let sec = gtk::gio::Menu::new();
        sec.append(Some("새 문서"), Some("app.new"));
        sec.append(Some("불러오기…"), Some("app.open"));
        file_menu.append_section(None, &sec);
        let sec = gtk::gio::Menu::new();
        sec.append(Some("저장하기"), Some("win.save"));
        sec.append(Some("다른 이름으로 저장하기…"), Some("win.save-as"));
        sec.append(Some("PDF로 저장하기…"), Some("win.export-pdf"));
        file_menu.append_section(None, &sec);
        let sec = gtk::gio::Menu::new();
        sec.append(Some("인쇄…"), Some("win.print"));
        file_menu.append_section(None, &sec);
        let file_btn = gtk::MenuButton::builder()
            .label("파일")
            .menu_model(&file_menu)
            .build();
        file_btn.add_css_class("tab");
        file_btn.add_css_class("file-tab");

        // ---- 편집 (Hwp 2022's ribbon and 편집 menu, docs/03-hwp2022-ui.md)
        let table_rows = spin(1.0, 200.0, 1.0, 0, "줄 개수");
        table_rows.set_value(5.0);
        let table_cols = spin(1.0, 50.0, 1.0, 0, "칸 개수");
        table_cols.set_value(5.0);
        let table_button = |rows: &gtk::SpinButton, cols: &gtk::SpinButton, label: &str| {
            let make = gtk::Button::builder()
                .label("만들기")
                .action_name("win.insert-table")
                .css_classes(["suggested-action", "pill"])
                .build();
            let form = gtk::Grid::builder()
                .row_spacing(8)
                .column_spacing(10)
                .margin_top(10)
                .margin_bottom(10)
                .margin_start(10)
                .margin_end(10)
                .build();
            let title = gtk::Label::new(Some("표 만들기"));
            title.add_css_class("heading");
            form.attach(&title, 0, 0, 2, 1);
            form.attach(&gtk::Label::new(Some("줄 개수")), 0, 1, 1, 1);
            form.attach(rows, 1, 1, 1, 1);
            form.attach(&gtk::Label::new(Some("칸 개수")), 0, 2, 1, 1);
            form.attach(cols, 1, 2, 1, 1);
            form.attach(&make, 0, 3, 2, 1);
            let b = gtk::MenuButton::builder()
                .child(&big_content("table", label, true))
                .popover(
                    &gtk::Popover::builder()
                        .child(&{
                            let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
                            let g = table_grid(rows, cols);
                            g.set_margin_top(10);
                            g.set_margin_start(10);
                            g.set_margin_end(10);
                            v.append(&g);
                            v.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
                            v.append(&form);
                            v
                        })
                        .build(),
                )
                .tooltip_text("표 만들기")
                .build();
            b.add_css_class("big");
            b
        };
        let edit = row();
        edit.append(&big("cut", "오려\n두기", "오려 두기 (Ctrl+X)", "win.cut"));
        edit.append(&big("copy", "복사하기", "복사하기 (Ctrl+C)", "win.copy"));
        edit.append(&big("paste", "붙이기", "붙이기 (Ctrl+V)", "win.paste"));
        edit.append(&big(
            "format-copy",
            "모양\n복사",
            "모양 복사 (Alt+C): 고른 것 없이 누르면 복사, 글자를 고르고 누르면 붙임",
            "win.format-copy",
        ));
        edit.append(&pending("clear-codes", "조판 부호\n지우기", false));
        edit.append(&sep());
        edit.append(&big(
            "char-shape",
            "글자\n모양",
            "글자 모양 (Alt+L)",
            "win.char-shape",
        ));
        edit.append(&big(
            "para-shape",
            "문단\n모양",
            "문단 모양 (Alt+T)",
            "win.para-shape",
        ));
        edit.append(&big("style", "스타일", "스타일 (F6)", "win.style-dialog"));
        edit.append(&sep());
        edit.append(&big(
            "page-portrait",
            "세로",
            "용지 방향: 세로",
            "win.page-portrait",
        ));
        edit.append(&big(
            "page-landscape",
            "가로",
            "용지 방향: 가로",
            "win.page-landscape",
        ));
        edit.append(&big(
            "page-margins",
            "쪽\n여백",
            "쪽 여백 (편집 용지, F7)",
            "win.page-setup",
        ));
        edit.append(&pending("master-page", "바탕쪽", false));
        edit.append(&columns_button());
        edit.append(&sep());
        edit.append(&shape_button());
        edit.append(&big("picture", "그림", "그림 넣기", "win.insert-picture"));
        edit.append(&table_button(&table_rows, &table_cols, "표"));
        edit.append(&pending("chart", "차트", true));
        edit.append(&sep());
        edit.append(&pending("object-select", "개체\n선택", true));
        edit.append(&pending("object-protect", "개체\n보호", true));
        edit.append(&sep());
        edit.append(&big("find", "찾기", "찾기 (Ctrl+F)", "win.find"));
        edit.append(&big(
            "find-replace",
            "글자\n바꾸기",
            "찾아 바꾸기 (Ctrl+H)",
            "win.replace",
        ));
        edit.append(&pending("sort", "정렬", false));

        // each tab's ˅ menu, item for item as in Hwp 2022 (submenus › are not built yet)
        const NO: &str = "app.none";
        let edit_menu = tab_menu(&[
            &[("되돌리기", "win.undo"), ("다시 실행", "win.redo")],
            &[
                ("오려 두기", "win.cut"),
                ("복사하기", "win.copy"),
                ("붙이기", "win.paste"),
                ("골라 붙이기…", NO),
                ("모양 복사…", "win.format-copy"),
            ],
            &[
                ("지우기", "win.delete"),
                ("조판 부호 지우기…", NO),
                ("모두 선택", "win.select-all"),
            ],
            &[
                ("찾기…", "win.find"),
                ("글자 바꾸기…", "win.replace"),
                ("정렬…", NO),
            ],
            &[("고치기…", NO)],
            &[("OLE 연결…", NO), ("OLE 개체 속성…", NO)],
        ]);
        let view_menu = tab_menu(&[
            &[("확대/축소…", "win.zoom-dialog"), ("쪽 윤곽", NO)],
            &[
                ("표시/숨기기", NO),
                ("메모", NO),
                ("한자 발음", NO),
                ("격자", NO),
                ("개체 이동 안내선", NO),
                ("회색조", "win.view-gray"),
            ],
            &[("도구 상자", NO), ("작업 창", NO), ("문서 창", NO)],
            &[("편집 화면 나누기", NO), ("창 배열", NO), ("창 목록…", NO)],
        ]);
        let insert_menu = tab_menu(&[
            &[("도형", NO)],
            &[
                ("그림…", "win.insert-picture"),
                ("표…", "win.insert-table"),
                ("차트", NO),
                ("글상자", NO),
                ("멀티미디어", NO),
            ],
            &[("수식…", "win.equation")],
            &[("개체", NO), ("캡션 넣기", NO), ("문단 띠", NO)],
            &[
                ("입력 도우미", NO),
                ("채우기", NO),
                ("각주", "win.footnote"),
                ("미주", "win.endnote"),
                ("날짜/시간/파일 이름…", "win.date-time"),
                ("덧말 넣기…", NO),
                ("문서 끼워 넣기…", "win.insert-file"),
            ],
            &[("문자표…", "win.char-map"), ("한자 입력", NO)],
            &[
                ("메모", NO),
                ("상호 참조…", NO),
                ("책갈피…", "win.bookmark"),
                ("하이퍼링크…", "win.hyperlink"),
            ],
        ]);
        let format_menu = tab_menu(&[
            &[
                ("글자 모양…", "win.char-shape"),
                ("문단 모양…", "win.para-shape"),
                ("문단 첫 글자 장식…", NO),
            ],
            &[
                ("문단 번호 모양…", NO),
                ("문단 번호 적용/해제", "win.list-number"),
                ("글머리표 적용/해제", "win.list-bullet"),
            ],
            &[("개요 번호 모양…", NO), ("개요 적용/해제", "win.outline")],
            &[
                ("한 수준 증가", "win.level-up"),
                ("한 수준 감소", "win.level-down"),
            ],
            &[("스타일…", "win.style-dialog"), ("스타일마당…", NO)],
            &[("개체 속성…", NO)],
        ]);
        let page_menu = tab_menu(&[
            &[
                ("편집 용지…", "win.page-setup"),
                ("글자 방향", NO),
                ("쪽 테두리/배경…", "win.page-border"),
                ("바탕쪽…", NO),
            ],
            &[
                ("머리말…", "win.header"),
                ("꼬리말…", "win.footer"),
                ("쪽 번호 매기기…", "win.page-number"),
                ("새 번호로 시작…", "win.new-number"),
                ("감추기…", "win.page-hide"),
                ("줄 번호", NO),
            ],
            &[
                ("쪽 나누기", "win.page-break"),
                ("단 나누기", "win.column-break"),
            ],
            &[
                ("단…", "win.columns-dialog"),
                ("다단 설정 나누기", NO),
                ("구역 설정…", NO),
                ("구역 나누기", NO),
            ],
            &[("쪽 복사하기", NO), ("쪽 붙이기", NO), ("쪽 지우기", NO)],
            &[("원고지…", NO), ("라벨", NO)],
        ]);
        let security_menu = tab_menu(&[
            &[("문서 암호 설정…", NO), ("문서 암호 변경/해제…", NO)],
            &[
                ("배포용 문서로 저장…", NO),
                ("배포용 문서 편집…", NO),
                ("배포용 문서 암호 변경/해제…", NO),
            ],
            &[("개인 정보 보호", NO)],
            &[("문서 보안 설정…", NO)],
        ]);
        let review_menu = tab_menu(&[
            &[("교정 부호", NO)],
            &[("변경 내용 추적", NO), ("변경 내용 표시 설정…", NO)],
            &[("문서 이력 관리…", NO), ("문서 비교…", NO)],
            &[("새 메모", NO), ("메모 모양…", NO), ("모든 메모 표시", NO)],
        ]);
        let tools_menu = tab_menu(&[
            &[
                ("맞춤법…", NO),
                ("한자 사전…", NO),
                ("유의어/반의어 사전…", NO),
                ("번역", NO),
                ("빠른 교정", NO),
            ],
            &[
                ("메일 머지", NO),
                ("스크립트 매크로", NO),
                ("차례/색인", NO),
                ("참고 문헌", NO),
                ("블록 계산", NO),
                ("문서 찾기…", NO),
                ("개인 정보 바꾸기", NO),
            ],
            &[("프레젠테이션", "win.presentation")],
            &[("글자판", NO)],
            &[
                ("추가 기능 설정…", NO),
                ("사용자 설정…", NO),
                ("환경 설정…", NO),
                ("스킨 설정…", NO),
            ],
        ]);

        // ---- 보기 (Hwp 2022: display options, then windows, then zoom)
        let zoom_label = gtk::Label::new(Some("100%"));
        zoom_label.set_width_chars(5);
        zoom_label.add_css_class("numeric");
        let view = row();
        fill(
            &view,
            &[
                P("sys:view-paged", "쪽\n윤곽", false),
                S,
                B("sys:view-reveal", "컬러", "컬러로 보기", "win.view-color"),
                B(
                    "sys:view-conceal",
                    "회색조",
                    "회색조로 보기 (인쇄·저장에는 영향 없음)",
                    "win.view-grayscale",
                ),
                P("sys:emoji-symbols", "사용자\n색", true),
                S,
            ],
        );
        view.append(&checks(&[
            ("문단 부호", false),
            ("조판 부호", false),
            ("교정 부호", true),
            ("그림", true),
            ("투명 선", false),
            ("메모", true),
            ("상황 선", true),
            ("문서 탭", true),
        ]));
        fill(
            &view,
            &[
                P("sys:view-grid", "격자", true),
                P("object-select", "개체 이동\n안내선", true),
                S,
                P("sys:accessories-dictionary", "한자\n발음 표시", true),
                S,
                P("sys:view-dual", "작업\n창", true),
                S,
                P("sys:view-dual", "편집 화면\n나누기", true),
                P("sys:window-new", "창\n배열", true),
                P("sys:window-new", "창\n전환", true),
                S,
                B("zoom-out", "축소", "축소 (Ctrl+-)", "win.zoom-out"),
                B("zoom-in", "확대", "확대 (Ctrl+=)", "win.zoom-in"),
                S,
                B("zoom-100", "100%", "원래 크기 (Ctrl+0)", "win.zoom-reset"),
            ],
        );
        view.append(&column(&[
            small_labeled(
                "fit-page",
                "쪽 맞춤",
                "한 쪽이 창에 다 보이게",
                "win.zoom-fit-page",
            ),
            small_labeled(
                "fit-width",
                "폭 맞춤",
                "쪽 너비를 창에 맞춤",
                "win.zoom-fit-width",
            ),
        ]));

        // ---- 입력
        let ins_rows = spin(1.0, 200.0, 1.0, 0, "줄 개수");
        let ins_cols = spin(1.0, 50.0, 1.0, 0, "칸 개수");
        table_rows
            .bind_property("value", &ins_rows, "value")
            .bidirectional()
            .sync_create()
            .build();
        table_cols
            .bind_property("value", &ins_cols, "value")
            .bidirectional()
            .sync_create()
            .build();
        let insert = row();
        insert.append(&shape_button());
        fill(
            &insert,
            &[S, B("picture", "그림", "그림 넣기", "win.insert-picture")],
        );
        insert.append(&table_button(&ins_rows, &ins_cols, "표"));
        fill(
            &insert,
            &[
                P("chart", "차트", true),
                P("sys:insert-text", "글맵시", true),
                B(
                    "sys:accessories-calculator",
                    "수식",
                    "수식 (Alt+Shift+E): LaTeX로 입력",
                    "win.equation",
                ),
                P("sys:insert-object", "OLE\n개체", false),
                S,
                P("sys:document-edit", "양식\n개체", true),
                P("sys:insert-text", "누름틀", false),
                S,
                B("sys:x-office-document", "각주", "각주", "win.footnote"),
                B("sys:x-office-document", "미주", "미주", "win.endnote"),
                P("memo", "메모", false),
                P("proof", "교정\n부호", true),
                S,
                B("sys:bookmark-new", "책갈피", "책갈피", "win.bookmark"),
                P("sys:insert-link", "상호\n참조", false),
                S,
                B(
                    "sys:insert-link",
                    "하이퍼링크",
                    "하이퍼링크 (고른 글자를 웹 주소로 연결)",
                    "win.hyperlink",
                ),
                S,
                B(
                    "sys:accessories-character-map",
                    "문자표",
                    "문자표 (Ctrl+F10)",
                    "win.char-map",
                ),
            ],
        );
        insert.append(&column(&[
            pending_small("sys:input-keyboard", "입력 도우미", true),
            pending_small("sys:accessories-dictionary", "한자 입력", true),
        ]));

        // ---- 서식 (font, size and B/I/U/S stay in the 서식 도구 상자 right below)
        let format = row();
        let styles = gtk::Grid::builder()
            .column_spacing(2)
            .row_spacing(0)
            .valign(gtk::Align::Center)
            .build();
        for (i, name) in ["바탕글", "본문", "개요 1", "개요 2"]
            .into_iter()
            .enumerate()
        {
            let b = gtk::Button::builder()
                .label(name)
                .tooltip_text(format!("스타일: {name}"))
                .action_name("win.style-name")
                .build();
            b.set_action_target_value(Some(&name.to_variant()));
            b.add_css_class("flat-small");
            b.add_css_class("style-item");
            b.set_focus_on_click(false);
            styles.attach(&b, (i % 2) as i32, (i / 2) as i32, 1, 1);
        }
        format.append(&styles);
        format.append(&column(&[
            pending_small("style", "스타일 추가", false),
            pending_small("table-gallery", "스타일마당", false),
        ]));
        fill(
            &format,
            &[
                S,
                B(
                    "char-shape",
                    "글자\n모양",
                    "글자 모양 (Alt+L)",
                    "win.char-shape",
                ),
                S,
                B(
                    "para-shape",
                    "문단\n모양",
                    "문단 모양 (Alt+T)",
                    "win.para-shape",
                ),
                S,
            ],
        );
        let align_menu = gtk::gio::Menu::new();
        for (label, action) in [
            ("양쪽 정렬", "win.align-justify"),
            ("왼쪽 정렬", "win.align-left"),
            ("가운데 정렬", "win.align-center"),
            ("오른쪽 정렬", "win.align-right"),
            ("배분 정렬", "win.align-distribute"),
            ("나눔 정렬", "win.align-split"),
        ] {
            align_menu.append(Some(label), Some(action));
        }
        let align_btn = gtk::MenuButton::builder()
            .child(&big_content("align-justify", "문단\n정렬", true))
            .menu_model(&align_menu)
            .tooltip_text("문단 정렬")
            .build();
        align_btn.add_css_class("big");
        format.append(&align_btn);
        format.append(&sep());
        let outline_menu = gtk::gio::Menu::new();
        let levels = gtk::gio::Menu::new();
        for n in 0..7 {
            let item = gtk::gio::MenuItem::new(Some(&format!("개요 {}", n + 1)), None);
            item.set_action_and_target_value(Some("win.outline-level"), Some(&n.to_variant()));
            levels.append_item(&item);
        }
        outline_menu.append_section(None, &levels);
        outline_menu.append(Some("개요 적용/해제"), Some("win.outline"));
        let outline_btn = gtk::MenuButton::builder()
            .child(&big_content("sys:view-list-ordered", "개요", true))
            .menu_model(&outline_menu)
            .tooltip_text("개요 (Ctrl+Insert)")
            .build();
        outline_btn.add_css_class("big");
        format.append(&outline_btn);
        let bullet_menu = gtk::gio::Menu::new();
        let shapes = gtk::gio::Menu::new();
        for ch in crate::window::BULLETS {
            let item = gtk::gio::MenuItem::new(Some(&format!("{ch}  글머리표")), None);
            item.set_action_and_target_value(
                Some("win.bullet"),
                Some(&ch.to_string().to_variant()),
            );
            shapes.append_item(&item);
        }
        bullet_menu.append_section(None, &shapes);
        bullet_menu.append(Some("글머리표 적용/해제"), Some("win.list-bullet"));
        let number_menu = gtk::gio::Menu::new();
        number_menu.append(Some("문단 번호 적용/해제"), Some("win.list-number"));
        let lists = gtk::Box::new(gtk::Orientation::Vertical, 0);
        lists.set_valign(gtk::Align::Center);
        lists.append(&small_menu(
            "sys:view-list-bullet",
            "글머리표",
            "글머리표 (Ctrl+Shift+Delete)",
            &bullet_menu,
        ));
        lists.append(&small_menu(
            "sys:view-list-ordered",
            "문단 번호",
            "문단 번호 (Ctrl+Shift+Insert)",
            &number_menu,
        ));
        format.append(&lists);
        format.append(&sep());
        format.append(&column(&[
            small_labeled(
                "sys:format-indent-more",
                "한 수준 증가",
                "한 수준 증가 (Ctrl+Num -)",
                "win.level-up",
            ),
            small_labeled(
                "sys:format-indent-less",
                "한 수준 감소",
                "한 수준 감소 (Ctrl+Num +)",
                "win.level-down",
            ),
        ]));

        // ---- 쪽
        let page = row();
        fill(
            &page,
            &[
                B(
                    "sys:document-page-setup",
                    "편집\n용지",
                    "편집 용지 (F7)",
                    "win.page-setup",
                ),
                B(
                    "page-portrait",
                    "세로",
                    "용지 방향: 세로",
                    "win.page-portrait",
                ),
                B(
                    "page-landscape",
                    "가로",
                    "용지 방향: 가로",
                    "win.page-landscape",
                ),
                B(
                    "page-margins",
                    "쪽\n여백",
                    "쪽 여백 (편집 용지, F7)",
                    "win.page-setup",
                ),
                S,
                P("sys:format-text-direction-ltr", "글자\n방향", true),
                S,
                P("master-page", "바탕쪽", false),
                B("header", "머리말", "머리말 (양쪽)", "win.header"),
                B("footer", "꼬리말", "꼬리말 (양쪽)", "win.footer"),
                B(
                    "page-border",
                    "쪽 테두리/\n배경",
                    "쪽 테두리/배경",
                    "win.page-border",
                ),
                B(
                    "page-number",
                    "쪽 번호\n매기기",
                    "쪽 번호 매기기",
                    "win.page-number",
                ),
                B(
                    "page-number",
                    "새 번호로\n시작",
                    "새 번호로 시작",
                    "win.new-number",
                ),
                B(
                    "sys:view-conceal",
                    "현재 쪽만\n감추기",
                    "현재 쪽만 감추기",
                    "win.page-hide",
                ),
                P("sys:view-list-ordered", "줄\n번호", true),
                S,
                B(
                    "page-break",
                    "쪽\n나누기",
                    "쪽 나누기 (Ctrl+Enter)",
                    "win.page-break",
                ),
                B(
                    "columns",
                    "단\n나누기",
                    "단 나누기 (Ctrl+Shift+Enter)",
                    "win.column-break",
                ),
                P("table-delete", "쪽\n지우기", false),
                S,
            ],
        );
        page.append(&columns_button());
        fill(
            &page,
            &[
                P("columns", "다단 설정\n나누기", false),
                S,
                P("section", "구역\n설정", false),
                P("section", "구역\n나누기", false),
                S,
                P("sys:view-grid", "라벨", true),
                P("sys:view-grid", "원고지", false),
            ],
        );

        // ---- 보안
        let security = row();
        fill(
            &security,
            &[
                P("sys:dialog-password", "문서\n암호 설정", false),
                P("sys:dialog-password", "문서 암호\n변경/해제", false),
                S,
                P("sys:document-save-as", "배포용\n문서로 저장", false),
                P("sys:document-edit", "배포용\n문서 편집", false),
                P("sys:dialog-password", "배포용 문서\n암호 변경/해제", false),
                S,
                P("sys:changes-prevent", "선택\n글자 보호", false),
                P("sys:system-search", "개인 정보\n찾아서 보호", false),
                P("sys:changes-prevent", "개인 정보\n보호 해제", false),
                P("sys:dialog-password", "개인 정보\n보호 암호 변경", false),
                S,
                P("sys:security-high", "문서\n보안 설정", false),
            ],
        );

        // ---- 검토
        let review = row();
        fill(
            &review,
            &[
                P("proof", "교정\n부호", true),
                S,
                P("sys:document-revert", "문서\n이력 관리", false),
                P("sys:view-dual", "문서\n비교", false),
                S,
                P("sys:document-edit", "변경\n내용 추적", true),
                P("sys:x-office-document", "안내문에\n표시", false),
                P("sys:view-reveal", "변경\n내용 보기", true),
                S,
                P("sys:x-office-document", "원본", false),
                P("sys:x-office-document", "원본 및\n변경 내용", false),
                P("sys:x-office-document", "최종본 및\n변경 내용", false),
                P("sys:x-office-document", "최종본", false),
                S,
                P("sys:object-select", "적용", true),
                P("table-delete", "적용\n안 함", true),
                S,
                P("memo", "새\n메모", false),
                P("memo", "메모", true),
            ],
        );

        // ---- 도구 (Hancom's own online services are left out)
        let tools_tab = row();
        fill(
            &tools_tab,
            &[
                P("sys:view-list-ordered", "색인", true),
                P("sys:view-list-bullet", "제목\n차례", true),
                S,
                P("sys:insert-link", "인용", true),
                S,
                P("sys:accessories-calculator", "블록\n계산", true),
                S,
                P("sys:media-playback-start", "스크립트\n매크로", true),
                S,
                P("sys:tools-check-spelling", "맞춤법", true),
                P("sys:tools-check-spelling", "빠른\n교정", true),
                S,
                P("sys:accessories-dictionary", "번역", true),
                S,
                B(
                    "sys:x-office-presentation",
                    "프레젠테이션",
                    "프레젠테이션: 쪽을 한 장씩 전체 화면으로 (Esc 끝)",
                    "win.presentation",
                ),
                S,
                P("sys:input-keyboard", "글자판", true),
                P("sys:avatar-default", "사용자\n설정", false),
                P("sys:emblem-system", "환경\n설정", false),
                S,
                P("sys:avatar-default", "개인 정보\n바꾸기", true),
            ],
        );

        // ---- 표 디자인 (contextual), as in Hwp 2022; only 표 만들기 works so far
        let design = row();
        let d_rows = spin(1.0, 200.0, 1.0, 0, "줄 개수");
        let d_cols = spin(1.0, 50.0, 1.0, 0, "칸 개수");
        table_rows
            .bind_property("value", &d_rows, "value")
            .bidirectional()
            .sync_create()
            .build();
        table_cols
            .bind_property("value", &d_cols, "value")
            .bidirectional()
            .sync_create()
            .build();
        design.append(&table_button(&d_rows, &d_cols, "표\n만들기"));
        let pens = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pens.set_valign(gtk::Align::Center);
        pens.append(&pending_small("table-draw", "표 그리기", false));
        pens.append(&pending_small("table-erase", "표 지우개", false));
        design.append(&pens);
        design.append(&sep());
        design.append(&pending("chart", "차트\n만들기", false));
        design.append(&big(
            "table-props",
            "표\n속성",
            "표/셀 속성 (P)",
            "win.table-props",
        ));
        design.append(&sep());
        // which parts a 표마당 style applies to (Hwp's defaults)
        design.append(&checks(&[
            ("테두리", true),
            ("글자/문단 모양", true),
            ("셀 배경", true),
            ("회색조", false),
            ("제목 줄", true),
            ("마지막 줄", true),
            ("첫째 칸", true),
            ("마지막 칸", true),
        ]));
        design.append(&sep());
        // 테두리˅ draws the pen chosen with 테두리 종류·굵기·색 on the selected cells
        let kinds = gtk::gio::Menu::new();
        for (i, label) in crate::window::BORDER_KINDS.into_iter().enumerate() {
            let item = gtk::gio::MenuItem::new(Some(label), None);
            item.set_action_and_target_value(
                Some("win.border-kind"),
                Some(&(i as i32).to_variant()),
            );
            kinds.append_item(&item);
        }
        let widths = gtk::gio::Menu::new();
        for (i, label) in crate::window::BORDER_WIDTHS.into_iter().enumerate() {
            let item = gtk::gio::MenuItem::new(Some(label), None);
            item.set_action_and_target_value(
                Some("win.border-width"),
                Some(&(i as i32).to_variant()),
            );
            widths.append_item(&item);
        }
        let col1 = gtk::Box::new(gtk::Orientation::Vertical, 0);
        col1.set_valign(gtk::Align::Center);
        col1.append(&color_menu(
            "table-fill",
            "표 채우기",
            "표 채우기: 고른 셀의 배경 색",
            "win.cell-fill",
            Some("없음"),
        ));
        col1.append(&border_menu());
        col1.append(&pending_small("cell-shade", "셀 음영", true));
        design.append(&col1);
        let col2 = gtk::Box::new(gtk::Orientation::Vertical, 0);
        col2.set_valign(gtk::Align::Center);
        col2.append(&small_menu(
            "border-type",
            "테두리 종류",
            "테두리 종류",
            &kinds,
        ));
        col2.append(&small_menu(
            "border-width",
            "테두리 굵기",
            "테두리 굵기",
            &widths,
        ));
        col2.append(&color_menu(
            "border-color",
            "테두리 색",
            "테두리 색",
            "win.border-color",
            None,
        ));
        design.append(&col2);
        design.append(&sep());
        design.append(&pending("table-gallery", "표마당", false));

        // ---- 표 레이아웃 (contextual): our table commands
        let table = row();
        for (i, l, t, a) in [
            (
                "row-above",
                "위에 줄",
                "위쪽에 줄 추가하기",
                "win.table-row-above",
            ),
            (
                "row-below",
                "아래에 줄",
                "아래쪽에 줄 추가하기",
                "win.table-row-below",
            ),
            (
                "col-left",
                "왼쪽에 칸",
                "왼쪽에 칸 추가하기",
                "win.table-col-left",
            ),
            (
                "col-right",
                "오른쪽에 칸",
                "오른쪽에 칸 추가하기",
                "win.table-col-right",
            ),
        ] {
            table.append(&big(i, l, t, a));
        }
        table.append(&sep());
        table.append(&big(
            "row-delete",
            "줄 지우기",
            "줄 지우기",
            "win.table-row-delete",
        ));
        table.append(&big(
            "col-delete",
            "칸 지우기",
            "칸 지우기",
            "win.table-col-delete",
        ));
        table.append(&sep());
        table.append(&big(
            "cell-merge",
            "셀 합치기",
            "끌어서 고른 셀 합치기",
            "win.cell-merge",
        ));
        table.append(&big(
            "cell-split",
            "셀 나누기",
            "합쳐진 셀 나누기",
            "win.cell-split",
        ));
        table.append(&sep());
        table.append(&big(
            "col-narrow",
            "너비 줄이기",
            "칸 너비 줄이기 (Ctrl+←) — 경계만 옮기려면 Alt+← 또는 경계를 끌기",
            "win.table-col-narrow",
        ));
        table.append(&big(
            "col-wide",
            "너비 늘리기",
            "칸 너비 늘리기 (Ctrl+→)",
            "win.table-col-wide",
        ));
        table.append(&big(
            "row-short",
            "높이 줄이기",
            "줄 높이 줄이기 (Ctrl+↑)",
            "win.table-row-short",
        ));
        table.append(&big(
            "row-tall",
            "높이 늘리기",
            "줄 높이 늘리기 (Ctrl+↓)",
            "win.table-row-tall",
        ));
        table.append(&big(
            "col-equal",
            "너비를 같게",
            "모든 칸의 너비를 같게 (표 너비는 그대로)",
            "win.table-col-equal",
        ));
        table.append(&sep());
        table.append(&big(
            "table-delete",
            "표 지우기",
            "표 전체 지우기",
            "win.table-delete",
        ));

        let stack = gtk::Stack::builder()
            .hhomogeneous(false)
            .vhomogeneous(true)
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(120)
            .build();
        let panels = [
            ("edit", "편집", edit, Some(edit_menu)),
            ("view", "보기", view, Some(view_menu)),
            ("insert", "입력", insert, Some(insert_menu)),
            ("format", "서식", format, Some(format_menu)),
            ("page", "쪽", page, Some(page_menu)),
            ("security", "보안", security, Some(security_menu)),
            ("review", "검토", review, Some(review_menu)),
            ("tools", "도구", tools_tab, Some(tools_menu)),
            ("table-design", "표 디자인", design, None),
            ("table-layout", "표 레이아웃", table, None),
        ];
        let tab_row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        tab_row.add_css_class("menu-tabs");
        tab_row.append(&file_btn);
        let mut tabs: Vec<(String, gtk::ToggleButton)> = Vec::new();
        for (id, title, panel, menu) in panels {
            let s = gtk::ScrolledWindow::builder()
                .child(&panel)
                .vscrollbar_policy(gtk::PolicyType::Never)
                .hscrollbar_policy(gtk::PolicyType::External)
                .propagate_natural_height(true)
                .build();
            stack.add_named(&s, Some(id));
            let t = gtk::ToggleButton::with_label(title);
            t.add_css_class("tab");
            t.set_focus_on_click(false);
            if let Some((_, first)) = tabs.first() {
                t.set_group(Some(first));
            }
            let st = stack.clone();
            let name = id.to_owned();
            t.connect_toggled(move |t| {
                if t.is_active() {
                    st.set_visible_child_name(&name);
                }
            });
            // a folded ribbon opens again when a tab is clicked
            let st = stack.clone();
            t.connect_clicked(move |_| st.set_visible(true));
            if let Some(menu) = menu {
                // Hwp: the name switches the ribbon, the ˅ beside it opens the tab's menu
                let more = gtk::MenuButton::builder()
                    .menu_model(&menu)
                    .tooltip_text(format!("{title} 메뉴"))
                    .build();
                more.set_icon_name("pan-down-symbolic");
                more.add_css_class("tab-more");
                let pair = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                pair.add_css_class("tab-pair");
                pair.append(&t);
                pair.append(&more);
                tab_row.append(&pair);
            } else {
                tab_row.append(&t);
            }
            tabs.push((id.to_owned(), t));
        }
        let table_tabs = [tabs[8].1.clone(), tabs[9].1.clone()];
        for t in &table_tabs {
            t.add_css_class("contextual");
            t.set_visible(false);
        }
        tabs[0].1.set_active(true);
        let find_entry = gtk::SearchEntry::builder()
            .placeholder_text("찾을 내용")
            .width_chars(16)
            .hexpand(true)
            .halign(gtk::Align::End)
            .build();
        find_entry.add_css_class("menu-find");
        tab_row.append(&find_entry);
        // Hwp's ^ at the right end folds the ribbon away
        let fold = gtk::Button::builder()
            .icon_name("pan-up-symbolic")
            .tooltip_text("리본 접기")
            .build();
        fold.set_focus_on_click(false);
        let st = stack.clone();
        fold.connect_clicked(move |b| {
            let open = !st.is_visible();
            st.set_visible(open);
            b.set_icon_name(if open {
                "pan-up-symbolic"
            } else {
                "pan-down-symbolic"
            });
            b.set_tooltip_text(Some(if open {
                "리본 접기"
            } else {
                "리본 펴기"
            }));
        });
        let f = fold.clone();
        stack.connect_visible_notify(move |s| {
            f.set_icon_name(if s.is_visible() {
                "pan-up-symbolic"
            } else {
                "pan-down-symbolic"
            });
        });
        tab_row.append(&fold);

        let ribbon_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        ribbon_box.append(&tab_row);
        ribbon_box.append(&stack);
        let ribbon_pane = GlassPane::new(&ribbon_box, backdrop, 18.0);
        ribbon_pane.add_css_class("ribbon");

        // ---- 서식 도구 상자
        let fonts =
            gtk::StringList::new(&font_names.iter().map(String::as_str).collect::<Vec<_>>());
        let font = gtk::DropDown::builder()
            .model(&fonts)
            .enable_search(true)
            .tooltip_text("글꼴")
            .build();
        font.set_expression(Some(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            None::<gtk::Expression>,
            "string",
        )));
        font.set_size_request(128, -1);
        font.set_selected(gtk::INVALID_LIST_POSITION); // filled from the caret
        let size = spin(1.0, 300.0, 0.5, 1, "글자 크기 (pt)");
        size.set_value(10.0);
        let bold = text_toggle("<b>가</b>", "진하게 (Ctrl+B)");
        let italic = text_toggle("<i>가</i>", "기울임 (Ctrl+I)");
        let underline = text_toggle("<u>가</u>", "밑줄 (Ctrl+U)");
        let strike = text_toggle("<s>가</s>", "취소선");
        let color = gtk::ColorDialogButton::new(Some(
            gtk::ColorDialog::builder()
                .with_alpha(false)
                .title("글자 색")
                .build(),
        ));
        color.set_rgba(&gdk::RGBA::BLACK);
        color.set_tooltip_text(Some("글자 색"));
        color.set_focus_on_click(false);
        let align = [
            icon_toggle("align-justify", "양쪽 정렬"),
            icon_toggle("align-left", "왼쪽 정렬"),
            icon_toggle("align-center", "가운데 정렬"),
            icon_toggle("align-right", "오른쪽 정렬"),
            icon_toggle("align-distribute", "배분 정렬"),
            icon_toggle("align-split", "나눔 정렬"),
        ];
        for b in &align[1..] {
            b.set_group(Some(&align[0]));
        }
        let line_spacing = spin(50.0, 500.0, 10.0, 0, "줄 간격 (%)");
        line_spacing.set_value(160.0);
        let preview = gtk::Button::builder()
            .icon_name(icons::name("doc-preview"))
            .build();
        mark_pending(&preview, "미리 보기");
        let tools = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tools.add_css_class("format-tools");
        // Hwp 2022's 서식 도구 상자, left to right
        tools.append(&small("doc-new", "새 문서 (Ctrl+N)", "app.new"));
        tools.append(&small("doc-open", "불러오기 (Ctrl+O)", "app.open"));
        tools.append(&small("doc-save", "저장하기 (Ctrl+S)", "win.save"));
        tools.append(&sep());
        tools.append(&small("doc-print", "인쇄 (Ctrl+P)", "win.print"));
        tools.append(&preview);
        tools.append(&sep());
        tools.append(&small("undo", "되돌리기 (Ctrl+Z)", "win.undo"));
        tools.append(&small("redo", "다시 실행 (Ctrl+Shift+Z)", "win.redo"));
        tools.append(&sep());
        let styles = gtk::StringList::new(&["바탕글"]);
        let style = gtk::DropDown::builder()
            .model(&styles)
            .tooltip_text("스타일")
            .build();
        style.set_size_request(84, -1);
        tools.append(&style);
        tools.append(&pending_drop("대표", "언어별 글꼴", 64));
        tools.append(&font);
        tools.append(&size);
        tools.append(&unit("pt"));
        tools.append(&sep());
        tools.append(&linked(&[
            bold.upcast_ref(),
            italic.upcast_ref(),
            underline.upcast_ref(),
            strike.upcast_ref(),
        ]));
        tools.append(&color);
        tools.append(&sep());
        let aligns: Vec<&gtk::Widget> = align.iter().map(|b| b.upcast_ref()).collect();
        tools.append(&linked(&aligns));
        tools.append(&sep());
        tools.append(&line_spacing);
        tools.append(&unit("%"));
        let tools_scroll = gtk::ScrolledWindow::builder()
            .child(&tools)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hscrollbar_policy(gtk::PolicyType::External)
            .propagate_natural_height(true)
            .build();
        let toolbar_pane = GlassPane::new(&tools_scroll, backdrop, 16.0);
        toolbar_pane.add_css_class("toolbar");

        Ribbon {
            ribbon_pane,
            toolbar_pane,
            tabs,
            table_tabs,
            bold,
            italic,
            underline,
            strike,
            font,
            fonts,
            style,
            styles,
            size,
            color,
            align,
            line_spacing,
            find_entry,
            table_rows,
            table_cols,
            zoom_label,
        }
    }

    /// Show the contextual 표 디자인 · 표 레이아웃 tabs while the caret is in a table (like Hancom).
    pub fn set_table_context(&self, in_table: bool) {
        if self.table_tabs[0].is_visible() == in_table {
            return;
        }
        for t in &self.table_tabs {
            t.set_visible(in_table);
        }
        if !in_table && self.table_tabs.iter().any(|t| t.is_active()) {
            if let Some((_, edit)) = self.tabs.iter().find(|(id, _)| id == "edit") {
                edit.set_active(true);
            }
        }
    }

    /// Ids of the ribbon tabs, in order (for `HWPEDIT_UI_SHOTS`).
    pub fn tab_ids(&self) -> Vec<String> {
        self.tabs.iter().map(|(id, _)| id.clone()).collect()
    }

    /// Bring tab `id` to the front, showing a contextual tab if needed.
    pub fn select_tab(&self, id: &str) {
        if let Some((_, t)) = self.tabs.iter().find(|(i, _)| i == id) {
            t.set_visible(true);
            t.set_active(true);
        }
    }

    /// Select `name` in the font list, adding it if the document uses a font not installed here.
    pub fn show_font(&self, name: &str) {
        if name.is_empty() {
            return;
        }
        let n = self.fonts.n_items();
        let idx = (0..n)
            .find(|&i| self.fonts.string(i).is_some_and(|s| s == name))
            .unwrap_or_else(|| {
                self.fonts.append(name);
                n
            });
        self.font.set_selected(idx);
    }

    /// Formatting controls only make sense with a caret in the document.
    pub fn set_format_enabled(&self, on: bool) {
        for w in [
            self.font.upcast_ref::<gtk::Widget>(),
            self.style.upcast_ref(),
            self.size.upcast_ref(),
            self.bold.upcast_ref(),
            self.italic.upcast_ref(),
            self.underline.upcast_ref(),
            self.strike.upcast_ref(),
            self.color.upcast_ref(),
        ]
        .into_iter()
        .chain(self.align.iter().map(|b| b.upcast_ref()))
        .chain(std::iter::once(self.line_spacing.upcast_ref()))
        {
            w.set_sensitive(on);
        }
    }

    /// Fill 스타일 with the document's styles (when they changed) and select `current`.
    pub fn show_styles(&self, names: &[&str], current: Option<usize>) {
        let same = self.styles.n_items() as usize == names.len()
            && names
                .iter()
                .enumerate()
                .all(|(i, n)| self.styles.string(i as u32).is_some_and(|s| s == *n));
        if !same {
            self.styles.splice(0, self.styles.n_items(), names);
        }
        self.style
            .set_selected(current.map_or(gtk::INVALID_LIST_POSITION, |i| i as u32));
    }

    pub fn selected_font(&self) -> Option<String> {
        self.fonts
            .string(self.font.selected())
            .map(|s| s.to_string())
    }
}

/// Liquid-glass styling. Panes paint their own blurred backdrop, tint and rim
/// (see glass.rs); CSS adds shape, shadow and the controls' look.
pub const CSS: &str = r#"
window.hwpedit { background: #dfe5ee; }

/* the desk behind the pages: soft colour fields so the glass has something to refract */
.canvas {
  background-color: #e4e9f1;
  background-image:
    radial-gradient(circle at 12% 8%, alpha(#7aa7ff, 0.38), transparent 42%),
    radial-gradient(circle at 88% 4%, alpha(#ff9bd0, 0.28), transparent 38%),
    radial-gradient(circle at 70% 92%, alpha(#8de0c8, 0.30), transparent 40%);
}
.canvas scrollbar { background: none; border: none; }
.page { box-shadow: 0 1px 2px alpha(black, 0.10), 0 10px 32px alpha(#20304a, 0.18); border-radius: 2px; }

glass {
  border-radius: 18px;
  box-shadow: 0 1px 1px alpha(black, 0.06), 0 12px 32px alpha(#1d2a44, 0.16);
}
glass.toolbar, glass.status { border-radius: 16px; }
glass.find { border-radius: 16px; }

/* title bar inside glass */
glass headerbar {
  background: none; background-color: transparent; color: @window_fg_color;
  box-shadow: none; border: none; min-height: 44px;
}

/* controls on glass: flat until hovered, glassy pills when pressed/checked.
   Not in the header pane, whose window controls keep the theme's look (some themes
   draw them as coloured dots); its menu button is a menubutton and still matches. */
glass.ribbon button, glass.toolbar button, glass.status button, glass.find button,
glass menubutton > button, glass dropdown > button, glass colorbutton > button {
  background: none; border: none; box-shadow: none; border-radius: 10px; min-height: 28px;
}
glass.ribbon button:hover, glass.toolbar button:hover, glass.status button:hover, glass.find button:hover,
glass menubutton > button:hover, glass dropdown > button:hover, glass colorbutton > button:hover {
  background: alpha(white, 0.42);
  box-shadow: inset 0 1px alpha(white, 0.8), 0 1px 2px alpha(black, 0.08);
}
glass.ribbon button:active, glass.ribbon button:checked, glass.toolbar button:active, glass.toolbar button:checked,
glass.find button:checked, glass menubutton > button:checked {
  background: alpha(@accent_bg_color, 0.16);
  box-shadow: inset 0 1px alpha(white, 0.6), inset 0 0 0 1px alpha(@accent_bg_color, 0.30);
}
glass button.suggested-action { background: @accent_bg_color; color: @accent_fg_color; }
glass .linked > button { border-radius: 0; }
glass .linked > button:first-child { border-radius: 10px 0 0 10px; }
glass .linked > button:last-child { border-radius: 0 10px 10px 0; }
glass spinbutton, glass entry {
  background: alpha(white, 0.50); border-radius: 9px; box-shadow: inset 0 1px 2px alpha(black, 0.08); border: none; min-height: 28px;
}
glass separator { background: alpha(#1d2a44, 0.14); min-width: 1px; }

/* menu tabs: a pill strip; the current tab is a raised glass bubble */
.menu-tabs { margin: 6px 10px 2px 10px; }
.menu-tabs button.tab, .menu-tabs menubutton.tab > button { padding: 2px 14px; min-height: 26px; border-radius: 13px; font-weight: 600; }
.menu-tabs menubutton.file-tab > button { margin-right: 8px; }
.menu-tabs entry.menu-find, .menu-tabs searchentry.menu-find { min-height: 26px; border-radius: 13px; }
.menu-tabs button.tab:checked {
  background: alpha(white, 0.82);
  box-shadow: 0 1px 3px alpha(black, 0.14), inset 0 1px alpha(white, 1);
  color: @accent_color;
}
/* the ˅ beside 편집 opens the tab's menu, as in Hwp */
.menu-tabs .tab-pair menubutton.tab-more > button { min-width: 0; min-height: 26px; padding: 2px 4px; margin-left: 1px; border-radius: 13px; }
.menu-tabs .tab-pair menubutton.tab-more image { -gtk-icon-size: 10px; opacity: 0.7; }
.menu-tabs button.tab.contextual { color: #c2410c; }
.menu-tabs button.tab.contextual:checked { color: #c2410c; background: alpha(#fff1e6, 0.9); }

/* ribbon panel: big buttons */
.ribbon-panel { margin: 2px 8px 8px 8px; min-height: 58px; }
button.big, menubutton.big > button { padding: 2px 3px; min-width: 34px; border-radius: 9px; }
button.big label, menubutton.big > button label { font-size: 0.78em; line-height: 1.05; }
.drop-mark { opacity: 0.6; }
glass button.flat-small { min-height: 22px; padding: 1px 6px; }
glass button.flat-small label { font-size: 0.82em; }
/* disabled and not-yet-supported controls: a readable grey, as in Hwp (the theme's
   opacity filter on the whole button plus its label made them almost invisible) */
glass button:disabled, glass menubutton > button:disabled { filter: none; }
glass button:disabled image, glass button:disabled label,
glass button.pending image, glass button.pending label { opacity: 0.42; }
glass dropdown:disabled, glass checkbutton:disabled, glass checkbutton:disabled label,
glass dropdown:disabled label { filter: none; }
glass dropdown:disabled > button > box, glass checkbutton:disabled > check { opacity: 0.5; }
glass checkbutton:disabled > label { color: @window_fg_color; opacity: 0.5; }
glass checkbutton { min-height: 22px; }
glass checkbutton label { font-size: 0.82em; }
.caption-label { font-size: 0.76em; opacity: 0.7; }

/* 서식 도구 상자 */
.format-tools { margin: 5px 10px; }
.format-tools button, .format-tools colorbutton > button, .format-tools dropdown > button { min-width: 22px; padding: 2px 6px; }
.format-tools spinbutton > button { min-width: 18px; padding: 0 4px; }
.format-tools separator { margin-left: 2px; margin-right: 2px; }
.format-tools > image { opacity: 0.75; margin: 0 2px; }

/* status bar */
.status-bar { margin: 3px 12px; }
.status-bar label { font-size: 0.86em; }
.status-bar scale { min-width: 160px; }

/* dark (set from AdwStyleManager): smoked glass on a deep desk */
window.hwpedit.dark { background: #15181f; }
window.hwpedit.dark .canvas {
    background-color: #191c23;
    background-image:
      radial-gradient(circle at 12% 8%, alpha(#3d6bd6, 0.35), transparent 42%),
      radial-gradient(circle at 88% 4%, alpha(#b03f86, 0.25), transparent 38%),
      radial-gradient(circle at 70% 92%, alpha(#1f8f72, 0.25), transparent 40%);
  }
window.hwpedit.dark glass { box-shadow: 0 1px 1px alpha(black, 0.3), 0 12px 32px alpha(black, 0.45); }
window.hwpedit.dark glass.ribbon button:hover, window.hwpedit.dark glass.toolbar button:hover, window.hwpedit.dark glass.status button:hover, window.hwpedit.dark glass.find button:hover, window.hwpedit.dark glass menubutton > button:hover, window.hwpedit.dark glass dropdown > button:hover { background: alpha(white, 0.10); box-shadow: inset 0 1px alpha(white, 0.12); }
window.hwpedit.dark glass spinbutton, window.hwpedit.dark glass entry { background: alpha(white, 0.08); }
window.hwpedit.dark glass separator { background: alpha(white, 0.12); }
window.hwpedit.dark .menu-tabs button.tab:checked { background: alpha(white, 0.16); box-shadow: inset 0 1px alpha(white, 0.2); }
window.hwpedit.dark .menu-tabs button.tab.contextual:checked { background: alpha(#c2410c, 0.25); color: #fdba74; }
window.hwpedit.dark .menu-tabs button.tab.contextual { color: #fdba74; }
"#;
