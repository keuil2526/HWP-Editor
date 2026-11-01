//! Hwp 2022's 표 디자인 › 표 채우기 (cell fill colour), 테두리, 테두리 종류 / 굵기 / 색.
//!
//! As in Hwp, 테두리 종류·굵기·색 choose the pen and 테두리˅ draws it on the chosen
//! edges of the selected cells (or the caret's cell).

use super::features::{caption, form};
use super::*;
use hwpedit_engine::{BorderLine, BorderSides, TableProps};

/// 테두리˅, in Hwp 2022's grid order: (tooltip, target of win.cell-border). The two
/// diagonals of Hwp's last row are not offered yet.
pub(crate) const BORDER_SIDES: [(&str, &str); 10] = [
    ("바깥쪽 테두리", "outer"),
    ("모든 테두리", "all"),
    ("안쪽 테두리", "inner"),
    ("테두리 없음", "none"),
    ("위쪽 테두리", "top"),
    ("아래쪽 테두리", "bottom"),
    ("왼쪽 테두리", "left"),
    ("오른쪽 테두리", "right"),
    ("안쪽 가로 테두리", "inner-h"),
    ("안쪽 세로 테두리", "inner-v"),
];

/// 테두리 종류˅, as Hwp 2022 lists them; the index is rhwp's line type code.
pub(crate) const BORDER_KINDS: [&str; 12] = [
    "없음",
    "실선",
    "파선",
    "점선",
    "일점 쇄선",
    "이점 쇄선",
    "긴 파선",
    "원형 점선",
    "이중 실선",
    "얇고 굵은 이중선",
    "굵고 얇은 이중선",
    "얇고 굵고 얇은 삼중선",
];

/// 테두리 굵기˅: Hwp's widths, index = width code.
pub(crate) const BORDER_WIDTHS: [&str; 16] = [
    "0.1mm", "0.12mm", "0.15mm", "0.2mm", "0.25mm", "0.3mm", "0.4mm", "0.5mm", "0.6mm", "0.7mm",
    "1mm", "1.5mm", "2mm", "3mm", "4mm", "5mm",
];

/// The pen a new table's cells are drawn with in Hwp: solid, 0.12 mm, black.
const DEFAULT_PEN: (i32, i32, &str) = (1, 1, "#000000");

impl Editor {
    pub(super) fn install_cell_actions(self: &Rc<Self>) {
        let a = gio::SimpleAction::new("table-props", None);
        let weak = Rc::downgrade(self);
        a.connect_activate(move |_, _| {
            if let Some(ed) = weak.upgrade() {
                ed.table_props_dialog();
            }
        });
        self.win.add_action(&a);
        let weak = Rc::downgrade(self);
        let border = gio::SimpleAction::new("cell-border", Some(glib::VariantTy::STRING));
        border.connect_activate(move |_, v| {
            if let (Some(ed), Some(side)) =
                (weak.upgrade(), v.and_then(|v| v.str().map(str::to_owned)))
            {
                ed.apply_borders(&side);
            }
        });
        self.win.add_action(&border);

        // 표 채우기: "" is 없음
        let weak = Rc::downgrade(self);
        let shade = gio::SimpleAction::new("cell-fill", Some(glib::VariantTy::STRING));
        shade.connect_activate(move |_, v| {
            if let (Some(ed), Some(color)) =
                (weak.upgrade(), v.and_then(|v| v.str().map(str::to_owned)))
            {
                let color = (!color.is_empty()).then_some(color);
                ed.edit_cell_block(move |d, a, b| d.set_cell_fill(a, b, color.as_deref()));
            }
        });
        self.win.add_action(&shade);

        // the pen: stateful, so the menus show the current choice
        let kind = gio::SimpleAction::new_stateful(
            "border-kind",
            Some(glib::VariantTy::INT32),
            &DEFAULT_PEN.0.to_variant(),
        );
        let width = gio::SimpleAction::new_stateful(
            "border-width",
            Some(glib::VariantTy::INT32),
            &DEFAULT_PEN.1.to_variant(),
        );
        let color = gio::SimpleAction::new_stateful(
            "border-color",
            Some(glib::VariantTy::STRING),
            &DEFAULT_PEN.2.to_variant(),
        );
        for a in [&kind, &width, &color] {
            a.connect_activate(|a, v| {
                if let Some(v) = v {
                    a.set_state(v);
                }
            });
            self.win.add_action(a);
        }
    }

    pub(super) fn refresh_cell_actions(&self, in_cell: bool) {
        for a in [
            "cell-border",
            "cell-fill",
            "border-kind",
            "border-width",
            "border-color",
        ] {
            self.set_enabled(a, in_cell);
        }
    }

    fn pen(&self) -> BorderLine {
        let state = |name: &str| self.win.lookup_action(name).and_then(|a| a.state());
        BorderLine {
            kind: state("border-kind")
                .and_then(|v| v.get::<i32>())
                .unwrap_or(DEFAULT_PEN.0) as u8,
            width: state("border-width")
                .and_then(|v| v.get::<i32>())
                .unwrap_or(DEFAULT_PEN.1) as u8,
            color: state("border-color")
                .and_then(|v| v.get::<String>())
                .unwrap_or_else(|| DEFAULT_PEN.2.into()),
        }
    }

    fn apply_borders(self: &Rc<Self>, side: &str) {
        let (sides, line) = match side {
            "none" => (
                BorderSides::All,
                BorderLine {
                    kind: 0,
                    width: 0,
                    color: "#000000".into(),
                },
            ),
            s => {
                let sides = match s {
                    "outer" => BorderSides::Outer,
                    "inner" => BorderSides::Inner,
                    "top" => BorderSides::Top,
                    "bottom" => BorderSides::Bottom,
                    "left" => BorderSides::Left,
                    "right" => BorderSides::Right,
                    "inner-h" => BorderSides::InnerHorizontal,
                    "inner-v" => BorderSides::InnerVertical,
                    _ => BorderSides::All,
                };
                (sides, self.pen())
            }
        };
        self.edit_cell_block(move |d, a, b| d.set_cell_borders(a, b, sides, &line));
    }

    /// Run `f` on the selected block of cells: from the selection's anchor cell to the
    /// caret's, or the caret's cell alone.
    fn edit_cell_block(
        self: &Rc<Self>,
        f: impl FnOnce(&mut Document, Pos, Pos) -> Result<(), hwpedit_engine::EngineError>,
    ) {
        let (a, b) = {
            let st = self.state.borrow();
            let Some(p @ Pos::Cell { .. }) = st.caret else {
                return;
            };
            let a = st.anchor.filter(|a| same_table(*a, p)).unwrap_or(p);
            (a, p)
        };
        let result = {
            let mut st = self.state.borrow_mut();
            let Some(l) = st.loaded.as_mut() else { return };
            let before = l.doc.page_count();
            f(&mut l.doc, a, b).map(|_| before != l.doc.page_count())
        };
        self.finish_format(result);
    }

    /// 표/셀 속성: Hwp's dialog, 표 tab (쪽 경계에서 나눔, 제목 줄 자동 반복, 모든 셀의
    /// 안 여백) and 셀 tab (세로 정렬, 안 여백 지정, 제목 셀) for the selected cells.
    fn table_props_dialog(self: &Rc<Self>) {
        let got = {
            let st = self.state.borrow();
            let (Some(l), Some(p @ Pos::Cell { .. })) = (&st.loaded, st.caret) else {
                drop(st);
                return self.toast("표 안에 캐럿을 두세요");
            };
            l.doc.table_props(p).zip(l.doc.cell_props(p))
        };
        let Some((tp, cp)) = got else { return };
        let mm = |v: f64| {
            let s = gtk::SpinButton::with_range(0.0, 100.0, 0.1);
            s.set_digits(2);
            s.set_value((v * 100.0).round() / 100.0);
            s.set_width_chars(6);
            s
        };
        let margins = |grid: &gtk::Grid, row: i32, v: [f64; 4]| -> [gtk::SpinButton; 4] {
            let spins = v.map(mm);
            for (i, (label, s)) in ["왼쪽", "오른쪽", "위쪽", "아래쪽"]
                .iter()
                .zip(&spins)
                .enumerate()
            {
                let r = row + (i / 2) as i32;
                let c = (i % 2) as i32 * 3;
                grid.attach(&caption(label), c, r, 1, 1);
                grid.attach(s, c + 1, r, 1, 1);
                grid.attach(&gtk::Label::new(Some("mm")), c + 2, r, 1, 1);
            }
            spins
        };

        // 표 tab
        let table = form();
        table.attach(&caption("쪽 경계에서"), 0, 0, 1, 1);
        let split = gtk::DropDown::from_strings(&["나누지 않음", "셀 단위로 나눔", "나눔"]);
        split.set_selected(tp.page_break.min(2) as u32);
        table.attach(&split, 1, 0, 5, 1);
        let repeat = gtk::CheckButton::with_label("제목 줄 자동 반복");
        repeat.set_active(tp.repeat_header);
        table.attach(&repeat, 0, 1, 6, 1);
        table.attach(&caption("모든 셀의 안 여백"), 0, 2, 6, 1);
        let table_margins = margins(&table, 3, tp.margins_mm);

        // 셀 tab
        let cell = form();
        cell.attach(&caption("세로 정렬"), 0, 0, 1, 1);
        let valign = gtk::DropDown::from_strings(&["위", "가운데", "아래"]);
        valign.set_selected(cp.vertical_align.min(2) as u32);
        cell.attach(&valign, 1, 0, 5, 1);
        let header = gtk::CheckButton::with_label("제목 셀");
        header.set_active(cp.header);
        cell.attach(&header, 0, 1, 6, 1);
        let own = gtk::CheckButton::with_label("안 여백 지정");
        own.set_active(cp.own_margins);
        cell.attach(&own, 0, 2, 6, 1);
        let cell_margins = margins(&cell, 3, cp.margins_mm);
        let sync = {
            let spins = cell_margins.clone();
            move |on: bool| spins.iter().for_each(|s| s.set_sensitive(on))
        };
        sync(cp.own_margins);
        own.connect_toggled(move |o| sync(o.is_active()));

        let stack = gtk::Stack::new();
        stack.add_titled(&table, Some("table"), "표");
        stack.add_titled(&cell, Some("cell"), "셀");
        let switcher = gtk::StackSwitcher::builder()
            .stack(&stack)
            .halign(gtk::Align::Center)
            .build();
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.append(&switcher);
        v.append(&stack);
        let d = adw::AlertDialog::new(
            Some("표/셀 속성"),
            Some("셀 탭은 고른 셀(블록이 없으면 캐럿이 있는 셀)에 적용됩니다."),
        );
        d.set_extra_child(Some(&v));
        d.add_responses(&[("cancel", "취소"), ("ok", "설정")]);
        d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("ok"));
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            let Some(ed) = weak.upgrade() else { return };
            if r != "ok" {
                return;
            }
            let near = |a: f64, b: f64| (a - b).abs() < 0.005;
            let new_table = TableProps {
                page_break: split.selected() as u8,
                repeat_header: repeat.is_active(),
                margins_mm: [0, 1, 2, 3].map(|i| table_margins[i].value()),
            };
            let table_changed = new_table.page_break != tp.page_break
                || new_table.repeat_header != tp.repeat_header
                || (0..4).any(|i| !near(new_table.margins_mm[i], tp.margins_mm[i]));
            let va = valign.selected() as u8;
            let m = [0, 1, 2, 3].map(|i| cell_margins[i].value());
            let margins_changed = own.is_active() != cp.own_margins
                || (own.is_active() && (0..4).any(|i| !near(m[i], cp.margins_mm[i])));
            let cell_change = (
                (va != cp.vertical_align).then_some(va),
                margins_changed.then_some(own.is_active().then_some(m)),
                (header.is_active() != cp.header).then_some(header.is_active()),
            );
            ed.edit_cell_block(move |d, a, b| {
                if table_changed {
                    d.set_table_props(b, &new_table)?;
                }
                let (va, m, h) = cell_change;
                d.set_cell_props(a, b, va, m, h)
            });
        });
        d.present(Some(&self.win));
    }
}
