//! Ribbon commands of Hwp 2022's 쪽 and 입력 tabs (and 편집 › 모양 복사) that need a
//! dialog or a file: 편집 용지, 세로/가로, 단, 단 나누기, 현재 쪽만 감추기,
//! 머리말·꼬리말, 그림, 각주·미주, 책갈피, 하이퍼링크, 문자표.

use super::*;
use hwpedit_engine::{PageHide, PageSetup};

/// Paper sizes offered in 편집 용지 (width x height in mm, portrait).
const PAPERS: [(&str, f64, f64); 7] = [
    ("A4 (210 × 297mm)", 210.0, 297.0),
    ("A3 (297 × 420mm)", 297.0, 420.0),
    ("A5 (148 × 210mm)", 148.0, 210.0),
    ("B4 (257 × 364mm)", 257.0, 364.0),
    ("B5 (182 × 257mm)", 182.0, 257.0),
    ("Letter (215.9 × 279.4mm)", 215.9, 279.4),
    ("Legal (215.9 × 355.6mm)", 215.9, 355.6),
];

/// 문자표: common symbols, grouped as Hwp's 사용자 문자표 roughly does.
const SYMBOLS: [(&str, &str); 6] = [
    (
        "기호",
        "※☆★○●◎◇◆□■△▲▽▼→←↑↓↔〓◁◀▷▶♤♠♡♥♧♣⊙◈▣◐◑▒▤▥▨▧▦▩♨☏☎☜☞¶†‡↕↗↙↖↘♭♩♪♬㉿㈜№㏇™㏂㏘℡®ªº§",
    ),
    (
        "수학",
        "±×÷≠≤≥∞∴∵♂♀∠⊥⌒∂∇≡≒≪≫√∽∝∫∬∈∋⊆⊇⊂⊃∪∩∧∨¬⇒⇔∀∃°′″℃Å￠￡￥",
    ),
    (
        "원·괄호 숫자",
        "①②③④⑤⑥⑦⑧⑨⑩⑪⑫⑬⑭⑮⑴⑵⑶⑷⑸⑹⑺⑻⑼⑽㉠㉡㉢㉣㉤㉥㉦㉧㉨㉩㉪㉫㉬㉭㈀㈁㈂㈃㈄㈅㈆㈇",
    ),
    ("로마 숫자", "ⅠⅡⅢⅣⅤⅥⅦⅧⅨⅩⅰⅱⅲⅳⅴⅵⅶⅷⅸⅹ"),
    ("그리스", "ΑΒΓΔΕΖΗΘΙΚΛΜΝΞΟΠΡΣΤΥΦΧΨΩαβγδεζηθικλμνξοπρστυφχψω"),
    (
        "단위",
        "㎕㎖㎗ℓ㎘㏄㎣㎤㎥㎦㎙㎚㎛㎜㎝㎞㎟㎠㎡㎢㏊㎍㎎㎏㏏㎈㎉㏈㎧㎨㎰㎱㎲㎳㎴㎵㎶㎷㎸㎹",
    ),
];

fn mm_spin(value: f64, max: f64) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(0.0, max, 0.5);
    s.set_digits(1);
    s.set_value(value);
    s.set_width_chars(6);
    s
}

pub(super) fn form() -> gtk::Grid {
    gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(10)
        .margin_top(6)
        .build()
}

pub(super) fn caption(t: &str) -> gtk::Label {
    gtk::Label::builder().label(t).xalign(0.0).build()
}

impl Editor {
    pub(super) fn install_feature_actions(self: &Rc<Self>) {
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
        add("date-time", |ed| ed.date_time_dialog());
        add("insert-file", |ed| ed.insert_file_dialog());
        add("page-setup", |ed| ed.page_setup_dialog());
        add("page-portrait", |ed| ed.set_orientation(false));
        add("page-landscape", |ed| ed.set_orientation(true));
        add("columns-dialog", |ed| ed.columns_dialog());
        add("column-break", |ed| {
            ed.edit(|d, p, _| d.insert_column_break(p).map(Some))
        });
        add("page-hide", |ed| ed.page_hide_dialog());
        add("insert-picture", |ed| ed.insert_picture_dialog());
        add("bookmark", |ed| ed.bookmark_dialog());
        add("hyperlink", |ed| ed.hyperlink_dialog());
        add("char-map", |ed| ed.char_map_dialog());
        add("format-copy", |ed| ed.format_copy());
        add("footnote", |ed| ed.note_dialog(false));
        add("endnote", |ed| ed.note_dialog(true));
        add("header", |ed| ed.header_footer_dialog(true));
        add("footer", |ed| ed.header_footer_dialog(false));
        // 단 › 1단 / 2단 / 3단
        let cols = gio::SimpleAction::new("columns", Some(glib::VariantTy::INT32));
        let weak = Rc::downgrade(self);
        cols.connect_activate(move |_, v| {
            if let (Some(ed), Some(n)) = (weak.upgrade(), v.and_then(|v| v.get::<i32>())) {
                ed.edit(move |d, p, _| d.set_columns(p, n as u16, 8.0).map(|_| None));
            }
        });
        self.win.add_action(&cols);
    }

    /// Enable the commands for where the caret is (called from refresh_chrome).
    pub(super) fn refresh_feature_actions(&self, caret: bool, in_body: bool, selection: bool) {
        for a in [
            "page-setup",
            "page-portrait",
            "page-landscape",
            "columns",
            "columns-dialog",
            "char-map",
            "format-copy",
            "insert-picture",
        ] {
            self.set_enabled(a, caret);
        }
        for a in ["header", "footer"] {
            self.set_enabled(a, caret);
        }
        for a in [
            "column-break",
            "page-hide",
            "bookmark",
            "footnote",
            "endnote",
        ] {
            self.set_enabled(a, in_body);
        }
        self.set_enabled("hyperlink", selection);
    }

    fn current_page_setup(&self) -> Option<PageSetup> {
        let st = self.state.borrow();
        let section = match st.caret? {
            Pos::Body { section, .. } | Pos::Cell { section, .. } => section,
        };
        st.loaded.as_ref()?.doc.page_setup(section)
    }

    fn set_orientation(self: &Rc<Self>, landscape: bool) {
        let Some(mut p) = self.current_page_setup() else {
            return;
        };
        if p.landscape == landscape {
            return;
        }
        p.landscape = landscape;
        self.edit(move |d, pos, _| d.set_page_setup(pos, &p).map(|_| None));
    }

    /// 편집 용지 (F7): paper, orientation, margins.
    fn page_setup_dialog(self: &Rc<Self>) {
        let Some(p) = self.current_page_setup() else {
            return;
        };
        let g = form();
        let papers = gtk::StringList::new(
            &PAPERS
                .iter()
                .map(|(n, ..)| *n)
                .chain(std::iter::once("사용자 정의"))
                .collect::<Vec<_>>(),
        );
        let paper = gtk::DropDown::builder().model(&papers).build();
        let near = |a: f64, b: f64| (a - b).abs() < 0.6;
        let idx = PAPERS
            .iter()
            .position(|(_, w, h)| near(*w, p.width_mm) && near(*h, p.height_mm))
            .unwrap_or(PAPERS.len());
        paper.set_selected(idx as u32);
        let width = mm_spin(p.width_mm, 1000.0);
        let height = mm_spin(p.height_mm, 1000.0);
        let portrait = gtk::CheckButton::with_label("세로");
        let landscape = gtk::CheckButton::with_label("가로");
        landscape.set_group(Some(&portrait));
        if p.landscape {
            landscape.set_active(true)
        } else {
            portrait.set_active(true)
        }
        {
            let (w, h) = (width.clone(), height.clone());
            paper.connect_selected_notify(move |d| {
                if let Some((_, pw, ph)) = PAPERS.get(d.selected() as usize) {
                    w.set_value(*pw);
                    h.set_value(*ph);
                }
            });
        }
        let margins = [
            ("위쪽", p.margin_top_mm),
            ("아래쪽", p.margin_bottom_mm),
            ("왼쪽", p.margin_left_mm),
            ("오른쪽", p.margin_right_mm),
            ("머리말", p.margin_header_mm),
            ("꼬리말", p.margin_footer_mm),
            ("제본", p.margin_gutter_mm),
        ]
        .map(|(l, v)| (l, mm_spin(v, 500.0)));
        g.attach(&caption("용지 종류"), 0, 0, 1, 1);
        g.attach(&paper, 1, 0, 3, 1);
        g.attach(&caption("폭 (mm)"), 0, 1, 1, 1);
        g.attach(&width, 1, 1, 1, 1);
        g.attach(&caption("길이 (mm)"), 2, 1, 1, 1);
        g.attach(&height, 3, 1, 1, 1);
        let orient = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        orient.append(&portrait);
        orient.append(&landscape);
        g.attach(&caption("용지 방향"), 0, 2, 1, 1);
        g.attach(&orient, 1, 2, 3, 1);
        let head = caption("용지 여백 (mm)");
        head.add_css_class("heading");
        g.attach(&head, 0, 3, 4, 1);
        for (i, (label, spin)) in margins.iter().enumerate() {
            let (r, c) = (4 + i as i32 / 2, (i as i32 % 2) * 2);
            g.attach(&caption(label), c, r, 1, 1);
            g.attach(spin, c + 1, r, 1, 1);
        }
        let d = adw::AlertDialog::new(Some("편집 용지"), None);
        d.set_extra_child(Some(&g));
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
            let m = |i: usize| margins[i].1.value();
            let q = PageSetup {
                width_mm: width.value(),
                height_mm: height.value(),
                landscape: landscape.is_active(),
                margin_top_mm: m(0),
                margin_bottom_mm: m(1),
                margin_left_mm: m(2),
                margin_right_mm: m(3),
                margin_header_mm: m(4),
                margin_footer_mm: m(5),
                margin_gutter_mm: m(6),
            };
            ed.edit(move |d, pos, _| d.set_page_setup(pos, &q).map(|_| None));
        });
        d.present(Some(&self.win));
    }

    /// 단 › 단 설정: number of columns and the gap between them.
    fn columns_dialog(self: &Rc<Self>) {
        let g = form();
        let count = gtk::SpinButton::with_range(1.0, 10.0, 1.0);
        count.set_value(2.0);
        let gap = mm_spin(8.0, 100.0);
        g.attach(&caption("단 개수"), 0, 0, 1, 1);
        g.attach(&count, 1, 0, 1, 1);
        g.attach(&caption("단 사이 간격 (mm)"), 0, 1, 1, 1);
        g.attach(&gap, 1, 1, 1, 1);
        let d = adw::AlertDialog::new(Some("단 설정"), Some("이 구역의 모든 쪽에 적용됩니다."));
        d.set_extra_child(Some(&g));
        d.add_responses(&[("cancel", "취소"), ("ok", "설정")]);
        d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("ok"));
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            let Some(ed) = weak.upgrade() else { return };
            if r == "ok" {
                let (n, mm) = (count.value() as u16, gap.value());
                ed.edit(move |d, p, _| d.set_columns(p, n, mm).map(|_| None));
            }
        });
        d.present(Some(&self.win));
    }

    /// 현재 쪽만 감추기: what to hide on the page holding the caret.
    fn page_hide_dialog(self: &Rc<Self>) {
        let names = ["머리말", "꼬리말", "바탕쪽", "테두리", "배경", "쪽 번호"];
        let checks: Vec<gtk::CheckButton> = names
            .iter()
            .map(|n| gtk::CheckButton::with_label(n))
            .collect();
        let g = form();
        for (i, c) in checks.iter().enumerate() {
            g.attach(c, (i % 2) as i32, (i / 2) as i32, 1, 1);
        }
        let d = adw::AlertDialog::new(Some("감추기"), Some("캐럿이 있는 쪽에서만 감춥니다."));
        d.set_extra_child(Some(&g));
        d.add_responses(&[("cancel", "취소"), ("ok", "넣기")]);
        d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("ok"));
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            let Some(ed) = weak.upgrade() else { return };
            if r != "ok" {
                return;
            }
            let on = |i: usize| checks[i].is_active();
            let h = PageHide {
                header: on(0),
                footer: on(1),
                master_page: on(2),
                border: on(3),
                fill: on(4),
                page_number: on(5),
            };
            ed.edit(move |d, p, _| d.hide_on_page(p, h).map(|_| None));
        });
        d.present(Some(&self.win));
    }

    /// 그림 넣기: an image file at the caret, as wide as it is at 96 dpi but at most
    /// the text width.
    fn insert_picture_dialog(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("그림 (PNG, JPEG, GIF, BMP)"));
        for m in ["image/png", "image/jpeg", "image/gif", "image/bmp"] {
            filter.add_mime_type(m);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("그림 넣기")
            .filters(&filters)
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.win), gio::Cancellable::NONE, move |res| {
            let (Some(ed), Ok(file)) = (weak.upgrade(), res) else {
                return;
            };
            let Some(path) = file.path() else { return };
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => return ed.alert("그림을 읽을 수 없습니다", &e.to_string()),
            };
            let size = gdk::Texture::from_bytes(&glib::Bytes::from(&bytes))
                .map(|t| (t.width() as u32, t.height() as u32));
            let Ok(px) = size else {
                return ed.alert(
                    "그림을 열 수 없습니다",
                    "PNG, JPEG, GIF, BMP 그림만 넣을 수 있습니다.",
                );
            };
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png")
                .to_ascii_lowercase();
            let ext = if ext == "jpeg" { "jpg".to_owned() } else { ext };
            let text_mm = ed.current_page_setup().map_or(150.0, |p| {
                let paper = if p.landscape { p.height_mm } else { p.width_mm };
                paper - p.margin_left_mm - p.margin_right_mm - p.margin_gutter_mm
            });
            let width_mm = (px.0 as f64 * 25.4 / 96.0).min(text_mm.max(10.0));
            ed.edit(move |d, p, _| d.insert_picture(p, &bytes, &ext, px, width_mm).map(Some));
            ed.mark_after_object();
        });
    }

    /// 문서 끼워 넣기: the body of another HWP/HWPX file at the caret, with its formatting.
    /// The other file is only read.
    fn insert_file_dialog(self: &Rc<Self>) {
        if self.in_table() {
            return self.toast("표 안에는 문서를 끼워 넣을 수 없습니다");
        }
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("한글 문서 (HWP, HWPX)"));
        for p in ["*.hwp", "*.hwpx", "*.HWP", "*.HWPX"] {
            filter.add_pattern(p);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("문서 끼워 넣기")
            .filters(&filters)
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.win), gio::Cancellable::NONE, move |res| {
            let (Some(ed), Ok(file)) = (weak.upgrade(), res) else {
                return;
            };
            let Some(path) = file.path() else { return };
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => return ed.alert("문서를 읽을 수 없습니다", &e.to_string()),
            };
            ed.edit(move |d, p, _| d.insert_document(p, &bytes).map(Some));
        });
    }

    /// Ask for one line of text, then run `done` with it.
    fn ask_text(
        self: &Rc<Self>,
        heading: &str,
        body: &str,
        initial: &str,
        ok: &str,
        done: impl Fn(&Rc<Editor>, String) + 'static,
    ) {
        let entry = gtk::Entry::builder()
            .text(initial)
            .activates_default(true)
            .build();
        let d = adw::AlertDialog::new(Some(heading), Some(body));
        d.set_extra_child(Some(&entry));
        d.add_responses(&[("cancel", "취소"), ("ok", ok)]);
        d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("ok"));
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        let e = entry.clone();
        d.connect_response(None, move |_, r| {
            if let (Some(ed), "ok") = (weak.upgrade(), r) {
                done(&ed, e.text().trim().to_owned());
            }
        });
        d.present(Some(&self.win));
        entry.grab_focus();
    }

    /// 각주 / 미주: the note's text is asked for and put in with the number.
    fn note_dialog(self: &Rc<Self>, endnote: bool) {
        let what = if endnote { "미주" } else { "각주" };
        self.ask_text(
            what,
            &format!("캐럿 위치에 {what} 번호를 넣고, 아래 내용을 {what}에 씁니다."),
            "",
            "넣기",
            move |ed, text| {
                ed.edit(move |d, p, _| d.insert_note(p, endnote, &text).map(Some));
                ed.mark_after_object();
            },
        );
    }

    /// 머리말 / 꼬리말 (양쪽): one line of text for every page of the section.
    fn header_footer_dialog(self: &Rc<Self>, header: bool) {
        let what = if header { "머리말" } else { "꼬리말" };
        let current = {
            let st = self.state.borrow();
            match (&st.loaded, st.caret) {
                (Some(l), Some(p)) => l.doc.header_footer_text(p, header),
                _ => return,
            }
        };
        let body = format!(
            "이 구역의 모든 쪽(양쪽)에 들어갑니다.{}",
            if current.is_some() {
                " 지금 내용을 바꿉니다."
            } else {
                ""
            }
        );
        self.ask_text(
            what,
            &body,
            current.as_deref().unwrap_or(""),
            "설정",
            move |ed, text| {
                ed.edit(move |d, p, _| d.set_header_footer_text(p, header, &text).map(|_| None));
            },
        );
    }

    fn bookmark_dialog(self: &Rc<Self>) {
        let existing = self
            .state
            .borrow()
            .loaded
            .as_ref()
            .map(|l| l.doc.bookmarks())
            .unwrap_or_default();
        let body = if existing.is_empty() {
            "캐럿 위치에 책갈피를 넣습니다.".to_owned()
        } else {
            format!("이 문서의 책갈피: {}", existing.join(", "))
        };
        self.ask_text("책갈피", &body, "", "넣기", |ed, name| {
            if name.is_empty() {
                return ed.toast("책갈피 이름을 입력하세요");
            }
            ed.edit(move |d, p, _| d.add_bookmark(p, &name).map(|_| None));
        });
    }

    fn hyperlink_dialog(self: &Rc<Self>) {
        let Some(r) = self.selection() else {
            return self.toast("링크로 만들 글자를 먼저 고르세요");
        };
        self.ask_text(
            "하이퍼링크",
            "고른 글자를 웹 주소로 연결합니다 (http:// 또는 https://).",
            "https://",
            "넣기",
            move |ed, uri| {
                let r = r;
                ed.edit(move |d, _, _| d.insert_hyperlink(r, &uri).map(|_| None));
            },
        );
    }

    /// 문자표: click a symbol to insert it at the caret; the dialog stays open.
    fn char_map_dialog(self: &Rc<Self>) {
        let stack = gtk::Stack::new();
        let switcher = gtk::StackSwitcher::builder()
            .stack(&stack)
            .halign(gtk::Align::Center)
            .build();
        for (name, chars) in SYMBOLS {
            let flow = gtk::FlowBox::builder()
                .max_children_per_line(12)
                .min_children_per_line(12)
                .selection_mode(gtk::SelectionMode::None)
                .homogeneous(true)
                .build();
            for ch in chars.chars() {
                let b = gtk::Button::with_label(&ch.to_string());
                b.add_css_class("flat");
                let weak = Rc::downgrade(self);
                b.connect_clicked(move |_| {
                    if let Some(ed) = weak.upgrade() {
                        ed.insert(&ch.to_string());
                    }
                });
                flow.insert(&b, -1);
            }
            let scroll = gtk::ScrolledWindow::builder()
                .child(&flow)
                .min_content_height(220)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build();
            stack.add_titled(&scroll, Some(name), name);
        }
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.append(&switcher);
        v.append(&stack);
        let d = adw::AlertDialog::new(Some("문자표"), Some("누른 문자가 캐럿 위치에 들어갑니다."));
        d.set_extra_child(Some(&v));
        d.add_response("close", "닫기");
        d.set_close_response("close");
        d.present(Some(&self.win));
    }

    /// 날짜/시간/파일 이름: Hwp's 날짜/시간 형식 and 파일 이름, put in as plain text.
    /// (Hwp's 날짜/시간 코드 — a field that updates itself — is not offered.)
    fn date_time_dialog(self: &Rc<Self>) {
        let Ok(now) = glib::DateTime::now_local() else {
            return;
        };
        let path = self
            .state
            .borrow()
            .loaded
            .as_ref()
            .and_then(|l| l.file.as_ref().map(|f| f.path().to_path_buf()));
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .build();
        list.add_css_class("boxed-list");
        let mut items = date_time_texts(&now);
        if let Some(p) = &path {
            if let Some(n) = p.file_name() {
                items.push(n.to_string_lossy().into_owned());
            }
            items.push(p.to_string_lossy().into_owned());
        }
        for t in &items {
            let l = gtk::Label::builder()
                .label(t)
                .xalign(0.0)
                .margin_top(6)
                .margin_bottom(6)
                .margin_start(10)
                .margin_end(10)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build();
            list.append(&l);
        }
        list.select_row(list.row_at_index(0).as_ref());
        let d = adw::AlertDialog::new(
            Some("날짜/시간/파일 이름"),
            Some(if path.is_some() {
                "고른 내용을 캐럿 위치에 글자로 넣습니다."
            } else {
                "고른 내용을 캐럿 위치에 글자로 넣습니다. (저장하지 않은 문서라 파일 이름은 없습니다)"
            }),
        );
        d.set_extra_child(Some(&list));
        d.add_responses(&[("cancel", "취소"), ("ok", "넣기")]);
        d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("ok"));
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            if r != "ok" {
                return;
            }
            let Some(i) = list.selected_row().map(|r| r.index() as usize) else {
                return;
            };
            if let (Some(ed), Some(t)) = (weak.upgrade(), items.get(i)) {
                ed.insert(t);
            }
        });
        d.present(Some(&self.win));
    }

    /// 모양 복사 (Alt+C), as in Hwp: with no selection, remember the character and
    /// paragraph shape at the caret; with a selection, apply the remembered shape to it.
    fn format_copy(self: &Rc<Self>) {
        let copied = self.state.borrow().format_copy.clone();
        match (self.selection(), copied) {
            (Some(_), Some((c, p))) => {
                self.apply_format(CharFormat {
                    bold: Some(c.bold),
                    italic: Some(c.italic),
                    underline: Some(c.underline),
                    strike: Some(c.strike),
                    size_pt: Some(c.size_pt),
                    color: Some(c.color),
                    font: Some(c.font),
                });
                if let Some(p) = p {
                    self.apply_align(p.align);
                    self.apply_para(ParaFormat {
                        line_spacing: Some(p.line_spacing),
                        margin_left_pt: Some(p.margin_left_pt),
                        margin_right_pt: Some(p.margin_right_pt),
                        indent_pt: Some(p.indent_pt),
                        spacing_before_pt: Some(p.spacing_before_pt),
                        spacing_after_pt: Some(p.spacing_after_pt),
                        widow_orphan: Some(p.widow_orphan),
                        keep_with_next: Some(p.keep_with_next),
                        keep_lines: Some(p.keep_lines),
                        page_break_before: Some(p.page_break_before),
                        ..Default::default()
                    });
                }
                self.toast("모양을 붙였습니다");
            }
            _ => {
                let got = {
                    let st = self.state.borrow();
                    let (Some(l), Some(pos)) = (&st.loaded, st.caret) else {
                        return;
                    };
                    l.doc
                        .char_state(pos)
                        .ok()
                        .map(|c| (c, l.doc.para_state(pos)))
                };
                if let Some(g) = got {
                    self.state.borrow_mut().format_copy = Some(g);
                    self.toast(
                        "모양을 복사했습니다. 바꿀 글자를 고른 뒤 다시 모양 복사(Alt+C)를 누르세요",
                    );
                }
            }
        }
    }
}

/// Hwp's 날짜/시간 형식 choices for `now`.
fn date_time_texts(now: &glib::DateTime) -> Vec<String> {
    const DAYS: [&str; 7] = ["월", "화", "수", "목", "금", "토", "일"];
    let (y, m, d) = (now.year(), now.month(), now.day_of_month());
    let day = DAYS[(now.day_of_week() as usize).clamp(1, 7) - 1];
    let (h, min) = (now.hour(), now.minute());
    let (half, h12) = (if h < 12 { "오전" } else { "오후" }, (h + 11) % 12 + 1);
    vec![
        format!("{y}년 {m}월 {d}일"),
        format!("{y}년 {m}월 {d}일 {day}요일"),
        format!("{y}. {m}. {d}."),
        format!("{y}-{m:02}-{d:02}"),
        format!("{y}/{m:02}/{d:02}"),
        format!("{half} {h12}:{min:02}"),
        format!("{h:02}:{min:02}"),
        format!("{y}년 {m}월 {d}일 {half} {h12}시 {min}분"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_time_formats() {
        let t = glib::DateTime::from_local(2026, 11, 1, 15, 4, 0.0).unwrap();
        let v = date_time_texts(&t);
        assert_eq!(v[0], "2026년 11월 1일");
        assert_eq!(v[1], "2026년 11월 1일 일요일");
        assert_eq!(v[2], "2026. 11. 1.");
        assert_eq!(v[3], "2026-11-01");
        assert_eq!(v[5], "오후 3:04");
        assert_eq!(v[6], "15:04");
        let midnight = glib::DateTime::from_local(2026, 1, 5, 0, 30, 0.0).unwrap();
        assert_eq!(date_time_texts(&midnight)[5], "오전 12:30");
        assert_eq!(date_time_texts(&midnight)[1], "2026년 1월 5일 월요일");
    }
}
