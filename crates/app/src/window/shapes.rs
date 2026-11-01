//! Hwp 2022's 도형 (직사각형, 타원, 직선). 글상자 is not offered yet: rhwp writes a new
//! text box into HWP so that Hwp 2022 cannot open the file (the dialog handles it already).
//!
//! Hwp draws a shape by dragging; this version cannot move or resize floating objects
//! yet, so a shape is put in at the caret as a character (글자처럼 취급) at the size
//! given here, and flows with the text.

use super::*;
use hwpedit_engine::ShapeKind;

impl Editor {
    pub(super) fn install_shape_actions(self: &Rc<Self>) {
        let a = gio::SimpleAction::new("shape", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        a.connect_activate(move |_, v| {
            let kind = match v.and_then(|v| v.str()) {
                Some("rectangle") => ShapeKind::Rectangle,
                Some("ellipse") => ShapeKind::Ellipse,
                Some("line") => ShapeKind::Line,
                _ => return,
            };
            if let Some(ed) = weak.upgrade() {
                ed.shape_dialog(kind);
            }
        });
        self.win.add_action(&a);
    }

    pub(super) fn refresh_shape_actions(&self, in_body: bool) {
        self.set_enabled("shape", in_body);
    }

    fn shape_dialog(self: &Rc<Self>, kind: ShapeKind) {
        let (title, w, h) = match kind {
            ShapeKind::Rectangle => ("직사각형", 30.0, 20.0),
            ShapeKind::Ellipse => ("타원", 30.0, 20.0),
            ShapeKind::Line => ("직선", 40.0, 0.0),
            ShapeKind::TextBox => ("글상자", 60.0, 20.0),
        };
        let spin = |v: f64| {
            let s = gtk::SpinButton::with_range(1.0, 500.0, 1.0);
            s.set_digits(1);
            s.set_value(v);
            s
        };
        let (width, height) = (spin(w), spin(f64::max(h, 1.0)));
        let g = gtk::Grid::builder()
            .row_spacing(8)
            .column_spacing(10)
            .margin_top(6)
            .build();
        let label = |t: &str| gtk::Label::builder().label(t).xalign(0.0).build();
        g.attach(
            &label(if kind == ShapeKind::Line {
                "길이 (mm)"
            } else {
                "너비 (mm)"
            }),
            0,
            0,
            1,
            1,
        );
        g.attach(&width, 1, 0, 1, 1);
        if kind != ShapeKind::Line {
            g.attach(&label("높이 (mm)"), 0, 1, 1, 1);
            g.attach(&height, 1, 1, 1, 1);
        }
        let text = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(6)
            .bottom_margin(6)
            .left_margin(6)
            .right_margin(6)
            .build();
        if kind == ShapeKind::TextBox {
            let scroll = gtk::ScrolledWindow::builder()
                .child(&text)
                .min_content_height(80)
                .min_content_width(320)
                .build();
            scroll.add_css_class("card");
            g.attach(&label("내용"), 0, 2, 1, 1);
            g.attach(&scroll, 1, 2, 1, 1);
        }
        let d = adw::AlertDialog::new(
            Some(title),
            Some("캐럿 자리에 글자처럼 넣습니다. (끌어서 그리기·옮기기는 아직 지원하지 않습니다)"),
        );
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
            let (w, h) = (width.value(), height.value());
            let b = text.buffer();
            let content = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
            ed.edit(move |d, p, _| d.insert_shape(p, kind, w, h, &content).map(Some));
            ed.mark_after_object();
        });
        d.present(Some(&self.win));
    }
}
