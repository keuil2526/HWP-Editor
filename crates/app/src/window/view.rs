//! Hwp 2022's 보기 tab options that only change the screen: 컬러/회색조, 문단 부호,
//! 투명 선, and 확대/축소… . The file is never changed by them.

use super::*;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct ViewOptions {
    /// 회색조: pages drawn in grey.
    pub gray: bool,
    /// 문단 부호 (↵ at each paragraph end).
    pub paragraph_marks: bool,
    /// 투명 선: table borders of kind "none" drawn as faint lines.
    pub transparent_lines: bool,
}

/// 프레젠테이션's rendered page: (page, screen width, screen height) -> surface.
type SlideCache = HashMap<(usize, i32, i32), cairo::ImageSurface>;

impl ViewOptions {
    pub fn apply(&self, doc: &mut Document) {
        doc.set_view_marks(self.paragraph_marks, self.transparent_lines);
    }
}

impl Editor {
    pub(super) fn install_view_actions(self: &Rc<Self>) {
        // check boxes in the 보기 tab: stateful actions
        let toggle = |name: &str,
                      get: fn(&ViewOptions) -> bool,
                      set: fn(&mut ViewOptions, bool)| {
            let a =
                gio::SimpleAction::new_stateful(name, None, &get(&self.view.get()).to_variant());
            let weak = Rc::downgrade(self);
            a.connect_change_state(move |a, v| {
                let Some(on) = v.and_then(|v| v.get::<bool>()) else {
                    return;
                };
                a.set_state(&on.to_variant());
                if let Some(ed) = weak.upgrade() {
                    let mut o = ed.view.get();
                    set(&mut o, on);
                    ed.set_view(o);
                }
            });
            self.win.add_action(&a);
        };
        toggle(
            "view-paragraph-marks",
            |o| o.paragraph_marks,
            |o, v| o.paragraph_marks = v,
        );
        toggle(
            "view-transparent-lines",
            |o| o.transparent_lines,
            |o, v| o.transparent_lines = v,
        );
        toggle("view-gray", |o| o.gray, |o, v| o.gray = v);
        // 컬러 / 회색조 buttons set the 회색조 state
        for (name, gray) in [("view-color", false), ("view-grayscale", true)] {
            let a = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(ed) = weak.upgrade() {
                    ed.win.change_action_state("view-gray", &gray.to_variant());
                }
            });
            self.win.add_action(&a);
        }
        let a = gio::SimpleAction::new("presentation", None);
        let weak = Rc::downgrade(self);
        a.connect_activate(move |_, _| {
            if let Some(ed) = weak.upgrade() {
                ed.presentation();
            }
        });
        self.win.add_action(&a);
        let a = gio::SimpleAction::new("zoom-dialog", None);
        let weak = Rc::downgrade(self);
        a.connect_activate(move |_, _| {
            if let Some(ed) = weak.upgrade() {
                ed.zoom_dialog();
            }
        });
        self.win.add_action(&a);
    }

    fn set_view(self: &Rc<Self>, o: ViewOptions) {
        if self.view.replace(o) == o {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            if let Some(l) = st.loaded.as_mut() {
                o.apply(&mut l.doc);
            }
            // a page being rendered has the same SVG under 회색조, but not the same pixels:
            // drop those jobs so they are not taken for the new ones
            st.rendering.clear();
        }
        // redraw every page
        let z = self.state.borrow().zoom;
        self.set_zoom(z);
    }

    /// 확대/축소: Hwp's presets and a free percentage.
    fn zoom_dialog(self: &Rc<Self>) {
        let z = self.state.borrow().zoom;
        let pct = gtk::SpinButton::with_range(10.0, 500.0, 10.0);
        pct.set_digits(0);
        pct.set_value((z * 100.0).round());
        let presets = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(4)
            .min_children_per_line(4)
            .homogeneous(true)
            .build();
        for p in [50, 75, 100, 125, 150, 200, 300, 400] {
            let b = gtk::Button::with_label(&format!("{p}%"));
            let s = pct.clone();
            b.connect_clicked(move |_| s.set_value(p as f64));
            presets.insert(&b, -1);
        }
        let fit = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let page = gtk::Button::with_label("쪽 맞춤");
        let width = gtk::Button::with_label("폭 맞춤");
        fit.append(&page);
        fit.append(&width);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&gtk::Label::new(Some("배율")));
        row.append(&pct);
        row.append(&gtk::Label::new(Some("%")));
        let v = gtk::Box::new(gtk::Orientation::Vertical, 10);
        v.append(&presets);
        v.append(&row);
        v.append(&fit);
        let d = adw::AlertDialog::new(Some("확대/축소"), None);
        d.set_extra_child(Some(&v));
        d.add_responses(&[("cancel", "취소"), ("ok", "설정")]);
        d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("ok"));
        d.set_close_response("cancel");
        for (b, whole) in [(&page, true), (&width, false)] {
            let weak = Rc::downgrade(self);
            let d2 = d.clone();
            b.connect_clicked(move |_| {
                d2.close();
                if let Some(ed) = weak.upgrade() {
                    ed.zoom_fit(whole);
                }
            });
        }
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            if r == "ok" {
                if let Some(ed) = weak.upgrade() {
                    ed.set_zoom(pct.value() / 100.0);
                }
            }
        });
        d.present(Some(&self.win));
    }
}

impl Editor {
    /// 프레젠테이션: the document full screen, one page at a time, as in Hwp's 쇼.
    /// → ↓ Space PageDown or a click: next page; ← ↑ PageUp Backspace: previous;
    /// Home/End: first/last; Esc: back to editing.
    pub(super) fn presentation(self: &Rc<Self>) {
        let (svgs, start) = {
            let st = self.state.borrow();
            let Some(l) = st.loaded.as_ref() else { return };
            let svgs: Vec<String> = (0..l.doc.page_count())
                .filter_map(|p| l.doc.render_page_svg(p).ok())
                .collect();
            let start = st
                .caret_rect
                .as_ref()
                .map_or(0, |r| r.page as usize)
                .min(svgs.len().saturating_sub(1));
            (svgs, start)
        };
        if svgs.is_empty() {
            return;
        }
        let page = Rc::new(Cell::new(start));
        // rendered pages for the current screen size: (page, width, height) -> surface
        let cache: Rc<RefCell<SlideCache>> = Default::default();
        let area = gtk::DrawingArea::new();
        let gray = self.view.get().gray;
        {
            let (page, cache) = (page.clone(), cache.clone());
            area.set_draw_func(move |_, cr, w, h| {
                cr.set_source_rgb(0.0, 0.0, 0.0);
                let _ = cr.paint();
                let i = page.get();
                let Some((pw, ph)) = render::svg_size(&svgs[i]) else {
                    return;
                };
                let scale = (w as f64 / pw).min(h as f64 / ph);
                let mut cache = cache.borrow_mut();
                if !cache.contains_key(&(i, w, h)) {
                    cache.clear();
                    let surf =
                        render::rasterize_bgra(&svgs[i], scale).and_then(|(sw, sh, mut d)| {
                            if gray {
                                render::grayscale(&mut d);
                            }
                            render::surface_from_bgra(sw, sh, d)
                        });
                    match surf {
                        Ok(s) => {
                            cache.insert((i, w, h), s);
                        }
                        Err(_) => return,
                    }
                }
                let s = &cache[&(i, w, h)];
                let x = ((w - s.width()) / 2) as f64;
                let y = ((h - s.height()) / 2) as f64;
                let _ = cr.set_source_surface(s, x, y);
                let _ = cr.paint();
            });
        }
        let win = gtk::Window::builder()
            .transient_for(&self.win)
            .modal(true)
            .title("프레젠테이션")
            .child(&area)
            .build();
        let count = {
            let st = self.state.borrow();
            st.loaded
                .as_ref()
                .map_or(0, |l| l.doc.page_count() as usize)
        };
        let go = {
            let (page, area) = (page.clone(), area.clone());
            move |to: isize| {
                let to = to.clamp(0, count as isize - 1) as usize;
                if to != page.get() {
                    page.set(to);
                    area.queue_draw();
                }
            }
        };
        let keys = gtk::EventControllerKey::new();
        {
            let (go, page, w) = (go.clone(), page.clone(), win.clone());
            keys.connect_key_pressed(move |_, key, _, _| {
                use gdk::Key;
                let i = page.get() as isize;
                match key {
                    Key::Escape => w.close(),
                    Key::Right | Key::Down | Key::space | Key::Page_Down | Key::Return => go(i + 1),
                    Key::Left | Key::Up | Key::BackSpace | Key::Page_Up => go(i - 1),
                    Key::Home => go(0),
                    Key::End => go(isize::MAX / 2),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            });
        }
        win.add_controller(keys);
        let click = gtk::GestureClick::new();
        click.set_button(0);
        {
            let page = page.clone();
            click.connect_pressed(move |g, _, _, _| {
                let i = page.get() as isize;
                // left click: next; right click: previous
                go(if g.current_button() == 3 {
                    i - 1
                } else {
                    i + 1
                });
            });
        }
        area.add_controller(click);
        win.fullscreen();
        win.present();
    }
}
