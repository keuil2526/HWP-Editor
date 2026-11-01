//! 수식 (Alt+Shift+E): type LaTeX, see it drawn, put it in as a Hwp equation.
//!
//! The file keeps Hwp's own equation script, which every Hwp reads; the LaTeX is
//! converted on the way in (`hwpedit_engine::latex`). With the caret next to an
//! equation, or on double-clicking one, the dialog edits it, shown back as LaTeX.

use super::*;
use hwpedit_engine::latex;

impl Editor {
    pub(super) fn install_equation_actions(self: &Rc<Self>) {
        let a = gio::SimpleAction::new("equation", None);
        let weak = Rc::downgrade(self);
        a.connect_activate(move |_, _| {
            if let Some(ed) = weak.upgrade() {
                ed.equation_dialog(None);
            }
        });
        self.win.add_action(&a);
    }

    /// Double-click on the page: edit the equation under the point, if there is one.
    pub(super) fn edit_equation_at(self: &Rc<Self>, page: u32, x: f64, y: f64) -> bool {
        let hit = {
            let st = self.state.borrow();
            st.loaded
                .as_ref()
                .and_then(|l| l.doc.equation_at_point(page, x / st.zoom, y / st.zoom))
        };
        let Some((at, control)) = hit else {
            return false;
        };
        self.place(Some(at), false);
        self.equation_dialog(Some((at, control)));
        true
    }

    /// Put in an equation at the caret, or edit `target` (its paragraph position and
    /// control) — by default the one next to the caret.
    fn equation_dialog(self: &Rc<Self>, target: Option<(Pos, usize)>) {
        let (caret, existing) = {
            let st = self.state.borrow();
            let Some(l) = &st.loaded else { return };
            match target {
                Some((p, c)) => (p, l.doc.equation_script(p, c).map(|s| (c, s))),
                None => {
                    let Some(p) = st.caret else { return };
                    (p, l.doc.equation_near(p))
                }
            }
        };
        let initial = existing
            .as_ref()
            .map(|(_, script)| latex::from_hwp(script).unwrap_or_else(|| script.clone()))
            .unwrap_or_default();

        let input = gtk::TextView::builder()
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(6)
            .bottom_margin(6)
            .left_margin(6)
            .right_margin(6)
            .build();
        input.buffer().set_text(&initial);
        let input_scroll = gtk::ScrolledWindow::builder()
            .child(&input)
            .min_content_height(90)
            .min_content_width(460)
            .build();
        input_scroll.add_css_class("card");
        let preview = gtk::DrawingArea::builder()
            .content_height(110)
            .hexpand(true)
            .build();
        let status = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .selectable(true)
            .build();
        status.add_css_class("caption");
        let hint = gtk::Label::builder()
            .label("예: \\frac{a}{b}, x^{2}, \\sqrt[3]{x}, \\sum_{i=1}^{n}, \\alpha, \\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}")
            .xalign(0.0)
            .wrap(true)
            .build();
        hint.add_css_class("dim-label");
        hint.add_css_class("caption");
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.append(&gtk::Label::builder().label("LaTeX").xalign(0.0).build());
        v.append(&input_scroll);
        v.append(&hint);
        v.append(&gtk::Label::builder().label("미리 보기").xalign(0.0).build());
        v.append(&preview);
        v.append(&status);

        // the latest conversion, shared by the preview and the 넣기 button
        let script: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let svg: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let update = {
            let (script, svg, status, preview) =
                (script.clone(), svg.clone(), status.clone(), preview.clone());
            let weak = Rc::downgrade(self);
            move |text: &str| {
                let Some(ed) = weak.upgrade() else { return };
                match latex::to_hwp(text) {
                    Ok(c) if !c.script.is_empty() => {
                        let drawn = ed
                            .state
                            .borrow()
                            .loaded
                            .as_ref()
                            .and_then(|l| l.doc.equation_preview_svg(&c.script).ok());
                        let mut msg = format!("한글 수식: {}", c.script);
                        if !c.unknown.is_empty() {
                            msg.push_str(&format!(
                                "\n바꾸지 못한 명령(이름 그대로 넣음): {}",
                                c.unknown.join(", ")
                            ));
                        }
                        status.set_text(&msg);
                        *svg.borrow_mut() = drawn;
                        *script.borrow_mut() = Some(c.script);
                    }
                    Ok(_) => {
                        status.set_text("");
                        *svg.borrow_mut() = None;
                        *script.borrow_mut() = None;
                    }
                    Err(e) => {
                        status.set_text(&format!("LaTeX 오류: {e}"));
                        *svg.borrow_mut() = None;
                        *script.borrow_mut() = None;
                    }
                }
                preview.queue_draw();
            }
        };
        let svg_draw = svg.clone();
        preview.set_draw_func(move |area, cr, w, h| {
            let Some(svg) = svg_draw.borrow().clone() else {
                return;
            };
            let Some((sw, sh)) = render::svg_size(&svg).filter(|(w, h)| *w > 0.0 && *h > 0.0)
            else {
                return;
            };
            // as large as fits, at most twice the 10 pt size
            let scale = (w as f64 / sw).min(h as f64 / sh).min(2.0) * area.scale_factor() as f64;
            let Ok(surface) = render::rasterize(&svg, scale) else {
                return;
            };
            let k = 1.0 / area.scale_factor() as f64;
            let (iw, ih) = (surface.width() as f64 * k, surface.height() as f64 * k);
            cr.translate(
                ((w as f64 - iw) / 2.0).max(0.0),
                ((h as f64 - ih) / 2.0).max(0.0),
            );
            cr.scale(k, k);
            let _ = cr.set_source_surface(&surface, 0.0, 0.0);
            let _ = cr.paint();
        });
        update(&initial);
        let up = update.clone();
        input
            .buffer()
            .connect_changed(move |b| up(&b.text(&b.start_iter(), &b.end_iter(), false)));

        let (title, ok) = if existing.is_some() {
            ("수식 고치기", "고치기")
        } else {
            ("수식 넣기", "넣기")
        };
        let d = adw::AlertDialog::new(
            Some(title),
            Some("LaTeX 문법으로 입력합니다. 문서에는 한글 수식으로 저장됩니다."),
        );
        d.set_extra_child(Some(&v));
        d.add_responses(&[("cancel", "취소"), ("ok", ok)]);
        d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            let Some(ed) = weak.upgrade() else { return };
            if r != "ok" {
                return;
            }
            let Some(s) = script.borrow().clone() else {
                ed.toast("넣을 수식이 없습니다");
                return;
            };
            match existing.as_ref().map(|e| e.0) {
                Some(control) => {
                    ed.edit(move |d, _, _| {
                        d.set_equation_script(caret, control, &s)
                            .map(|_| Some(caret))
                    });
                    ed.mark_after_object();
                }
                None => {
                    ed.edit(move |d, p, _| d.insert_equation(p, &s).map(Some));
                    ed.mark_after_object();
                }
            }
        });
        d.present(Some(&self.win));
        input.grab_focus();
    }
}
