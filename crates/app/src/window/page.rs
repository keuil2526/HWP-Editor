//! Hwp 2022's 쪽 › 쪽 번호 매기기, 새 번호로 시작 and 쪽 테두리/배경.

use super::*;
use hwpedit_engine::{BorderLine, PageBorder, PageNumber};

/// 번호 위치, as Hwp's dialog names them: (label, pgnp position code).
const POSITIONS: [(&str, u8); 11] = [
    ("왼쪽 위", 1),
    ("가운데 위", 2),
    ("오른쪽 위", 3),
    ("왼쪽 아래", 4),
    ("가운데 아래", 5),
    ("오른쪽 아래", 6),
    ("바깥쪽 위", 7),
    ("바깥쪽 아래", 8),
    ("안쪽 위", 9),
    ("안쪽 아래", 10),
    ("쪽 번호 없음", 0),
];

/// 번호 모양: the shapes whose code means the same to rhwp's HWP and HWPX writers.
const SHAPES: [(&str, u8); 6] = [
    ("1,2,3", 0),
    ("①,②,③", 1),
    ("I,II,III", 2),
    ("i,ii,iii", 3),
    ("A,B,C", 4),
    ("a,b,c", 5),
];

fn labelled_dropdown(items: &[&str], selected: usize) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(items);
    d.set_selected(selected as u32);
    d
}

fn form_dialog(
    title: &str,
    body: &str,
    child: &impl IsA<gtk::Widget>,
    ok: &str,
) -> adw::AlertDialog {
    let d = adw::AlertDialog::new(Some(title), Some(body));
    d.set_extra_child(Some(child));
    d.add_responses(&[("cancel", "취소"), ("ok", ok)]);
    d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("ok"));
    d.set_close_response("cancel");
    d
}

impl Editor {
    pub(super) fn install_page_actions(self: &Rc<Self>) {
        type Handler = fn(&Rc<Editor>);
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
        add("page-number", |ed| ed.page_number_dialog());
        add("new-number", |ed| ed.new_number_dialog());
        add("page-border", |ed| ed.page_border_dialog());
    }

    pub(super) fn refresh_page_actions(&self, caret: bool) {
        for a in ["page-number", "new-number", "page-border"] {
            self.set_enabled(a, caret);
        }
    }

    fn caret_section(&self) -> Option<usize> {
        match self.state.borrow().caret? {
            Pos::Body { section, .. } | Pos::Cell { section, .. } => Some(section),
        }
    }

    /// 쪽 번호 매기기: 번호 위치 and 번호 모양 for the caret's section.
    fn page_number_dialog(self: &Rc<Self>) {
        let Some(section) = self.caret_section() else {
            return;
        };
        let current = self
            .state
            .borrow()
            .loaded
            .as_ref()
            .and_then(|l| l.doc.page_number(section));
        // Hwp's default: 가운데 아래, 1,2,3, 줄표 넣기
        let cur = current.unwrap_or(PageNumber {
            position: 5,
            format: 0,
            dash: true,
        });
        let pos = labelled_dropdown(
            &POSITIONS.map(|p| p.0),
            POSITIONS
                .iter()
                .position(|p| p.1 == cur.position)
                .unwrap_or(4),
        );
        let shape = labelled_dropdown(
            &SHAPES.map(|s| s.0),
            SHAPES.iter().position(|s| s.1 == cur.format).unwrap_or(0),
        );
        // the HWPX writer always stores a 줄표; until "off" can be saved there it stays on
        let dash = gtk::CheckButton::builder()
            .label("줄표 넣기 (- 1 -)")
            .active(true)
            .sensitive(false)
            .build();
        let g = gtk::Grid::builder()
            .row_spacing(8)
            .column_spacing(10)
            .margin_top(6)
            .build();
        for (i, (label, w)) in [
            ("번호 위치", pos.upcast_ref::<gtk::Widget>()),
            ("번호 모양", shape.upcast_ref()),
        ]
        .into_iter()
        .enumerate()
        {
            g.attach(
                &gtk::Label::builder().label(label).xalign(0.0).build(),
                0,
                i as i32,
                1,
                1,
            );
            g.attach(w, 1, i as i32, 1, 1);
        }
        g.attach(&dash, 1, 2, 1, 1);
        let d = form_dialog(
            "쪽 번호 매기기",
            "이 구역의 쪽에 번호를 넣습니다. 시작 번호는 「새 번호로 시작」에서 정합니다.",
            &g,
            "넣기",
        );
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            let Some(ed) = weak.upgrade() else { return };
            if r == "ok" {
                let n = PageNumber {
                    position: POSITIONS[pos.selected() as usize].1,
                    format: SHAPES[shape.selected() as usize].1,
                    dash: true,
                };
                ed.edit(move |d, p, _| d.set_page_number(p, &n).map(|_| None));
            }
        });
        d.present(Some(&self.win));
    }

    /// 새 번호로 시작 (쪽 번호): the page holding the caret gets `시작 번호`.
    fn new_number_dialog(self: &Rc<Self>) {
        let current = {
            let st = self.state.borrow();
            st.caret
                .and_then(|p| st.loaded.as_ref()?.doc.new_page_number_at(p))
        };
        let kinds = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let page = gtk::CheckButton::builder()
            .label("쪽 번호")
            .active(true)
            .build();
        kinds.append(&page);
        for other in [
            "각주 번호",
            "미주 번호",
            "그림 번호",
            "표 번호",
            "수식 번호",
        ] {
            let c = gtk::CheckButton::builder()
                .label(format!("{other} (아직 지원하지 않음)"))
                .sensitive(false)
                .build();
            c.set_group(Some(&page));
            kinds.append(&c);
        }
        let start = gtk::SpinButton::with_range(1.0, 9999.0, 1.0);
        start.set_value(current.unwrap_or(1) as f64);
        let g = gtk::Grid::builder()
            .row_spacing(8)
            .column_spacing(10)
            .margin_top(6)
            .build();
        g.attach(
            &gtk::Label::builder()
                .label("번호 종류")
                .xalign(0.0)
                .valign(gtk::Align::Start)
                .build(),
            0,
            0,
            1,
            1,
        );
        g.attach(&kinds, 1, 0, 1, 1);
        g.attach(
            &gtk::Label::builder().label("시작 번호").xalign(0.0).build(),
            0,
            1,
            1,
            1,
        );
        g.attach(&start, 1, 1, 1, 1);
        let d = form_dialog(
            "새 번호로 시작",
            "캐럿이 있는 쪽부터 이 번호로 셉니다.",
            &g,
            "넣기",
        );
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            let Some(ed) = weak.upgrade() else { return };
            if r == "ok" {
                let n = start.value() as u16;
                ed.edit(move |d, p, _| d.set_new_page_number(p, n).map(|_| None));
            }
        });
        d.present(Some(&self.win));
    }

    /// 쪽 테두리/배경 (양쪽, 모든 쪽): one line for all four sides, 위치, 배경 색.
    fn page_border_dialog(self: &Rc<Self>) {
        let Some(section) = self.caret_section() else {
            return;
        };
        let Some(cur) = self
            .state
            .borrow()
            .loaded
            .as_ref()
            .and_then(|l| l.doc.page_border(section))
        else {
            return;
        };
        let line = cur.borders.iter().find(|b| b.kind != 0).cloned();
        let kinds: Vec<&str> = BORDER_KINDS.to_vec();
        // Hwp's dialog opens with 없음, 0.1mm, black
        let kind = labelled_dropdown(
            &kinds,
            line.as_ref()
                .map_or(0, |l| l.kind as usize)
                .min(kinds.len() - 1),
        );
        let width = labelled_dropdown(
            &BORDER_WIDTHS,
            line.as_ref()
                .map_or(0, |l| l.width as usize)
                .min(BORDER_WIDTHS.len() - 1),
        );
        let color = gtk::ColorDialogButton::new(Some(
            gtk::ColorDialog::builder().with_alpha(false).build(),
        ));
        color.set_rgba(
            &gdk::RGBA::parse(line.as_ref().map_or("#000000", |l| l.color.as_str()))
                .unwrap_or(gdk::RGBA::BLACK),
        );
        let page_based = gtk::CheckButton::builder()
            .label("쪽 기준")
            .active(cur.page_based)
            .build();
        let paper_based = gtk::CheckButton::builder()
            .label("종이 기준")
            .active(!cur.page_based)
            .group(&page_based)
            .build();
        let spins: Vec<gtk::SpinButton> = cur
            .spacing_mm
            .iter()
            .map(|v| {
                let s = gtk::SpinButton::with_range(0.0, 100.0, 0.5);
                s.set_digits(2);
                s.set_value(*v);
                s
            })
            .collect();
        let header = gtk::CheckButton::builder()
            .label("머리말 포함")
            .active(cur.header_inside)
            .build();
        let footer = gtk::CheckButton::builder()
            .label("꼬리말 포함")
            .active(cur.footer_inside)
            .build();
        let use_fill = gtk::CheckButton::builder()
            .label("배경 색")
            .active(cur.fill.is_some())
            .build();
        let fill = gtk::ColorDialogButton::new(Some(
            gtk::ColorDialog::builder().with_alpha(false).build(),
        ));
        fill.set_rgba(
            &gdk::RGBA::parse(cur.fill.as_deref().unwrap_or("#ffffff")).unwrap_or(gdk::RGBA::WHITE),
        );

        let g = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(10)
            .margin_top(6)
            .build();
        let label = |t: &str| gtk::Label::builder().label(t).xalign(0.0).build();
        g.attach(&label("테두리 종류"), 0, 0, 1, 1);
        g.attach(&kind, 1, 0, 3, 1);
        g.attach(&label("굵기"), 0, 1, 1, 1);
        g.attach(&width, 1, 1, 3, 1);
        g.attach(&label("색"), 0, 2, 1, 1);
        g.attach(&color, 1, 2, 3, 1);
        let basis = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        basis.append(&paper_based);
        basis.append(&page_based);
        g.attach(&label("위치"), 0, 3, 1, 1);
        g.attach(&basis, 1, 3, 3, 1);
        for (i, name) in ["왼쪽", "오른쪽", "위쪽", "아래쪽"].iter().enumerate() {
            let (r, c) = (4 + (i / 2) as i32, (i % 2) as i32 * 2);
            g.attach(&label(&format!("{name} (mm)")), c, r, 1, 1);
            g.attach(&spins[i], c + 1, r, 1, 1);
        }
        g.attach(&header, 1, 6, 1, 1);
        g.attach(&footer, 3, 6, 1, 1);
        g.attach(&use_fill, 0, 7, 1, 1);
        g.attach(&fill, 1, 7, 3, 1);
        let d = form_dialog(
            "쪽 테두리/배경",
            "이 구역의 모든 쪽(양쪽)에 적용됩니다.",
            &g,
            "설정",
        );
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            let Some(ed) = weak.upgrade() else { return };
            if r != "ok" {
                return;
            }
            let hex = |c: gdk::RGBA| {
                format!(
                    "#{:02x}{:02x}{:02x}",
                    (c.red() * 255.0).round() as u8,
                    (c.green() * 255.0).round() as u8,
                    (c.blue() * 255.0).round() as u8
                )
            };
            let k = kind.selected() as u8;
            let l = BorderLine {
                kind: k,
                width: if k == 0 { 0 } else { width.selected() as u8 },
                color: hex(color.rgba()),
            };
            let b = PageBorder {
                borders: [l.clone(), l.clone(), l.clone(), l],
                fill: use_fill.is_active().then(|| hex(fill.rgba())),
                page_based: page_based.is_active(),
                spacing_mm: [
                    spins[0].value(),
                    spins[1].value(),
                    spins[2].value(),
                    spins[3].value(),
                ],
                header_inside: header.is_active(),
                footer_inside: footer.is_active(),
            };
            ed.edit(move |d, p, _| d.set_page_border(p, &b).map(|_| None));
        });
        d.present(Some(&self.win));
    }
}
