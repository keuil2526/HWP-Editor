//! Hwp 2022's 서식 › 스타일, 개요, 글머리표, 문단 번호 and 한 수준 증가/감소.

use super::*;
use hwpedit_engine::{EngineError, ListKind};

/// 글머리표˅: the bullets offered, as in Hwp's 글머리표 gallery.
pub(crate) const BULLETS: [char; 10] = ['●', '■', '◆', '▶', '○', '□', '◇', '▷', '★', '✓'];

impl Editor {
    pub(super) fn install_list_actions(self: &Rc<Self>) {
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
        add("list-number", |ed| ed.toggle_list(ListKind::Number, '●'));
        add("list-bullet", |ed| ed.toggle_list(ListKind::Bullet, '●'));
        add("outline", |ed| ed.toggle_list(ListKind::Outline, '●'));
        // 증가 moves a paragraph up toward 개요 1 / 바탕글, 감소 one level deeper (Hwp 2022)
        add("level-up", |ed| ed.change_level(-1));
        add("level-down", |ed| ed.change_level(1));
        add("style-dialog", |ed| ed.style_dialog());

        let typed = |name: &str, ty: &glib::VariantTy, f: fn(&Rc<Editor>, &glib::Variant)| {
            let a = gio::SimpleAction::new(name, Some(ty));
            let weak = Rc::downgrade(self);
            a.connect_activate(move |_, v| {
                if let (Some(ed), Some(v)) = (weak.upgrade(), v) {
                    f(&ed, v);
                }
            });
            self.win.add_action(&a);
        };
        // 스타일 목록 in the ribbon names its styles (the ids differ between documents)
        typed("style-name", glib::VariantTy::STRING, |ed, v| {
            if let Some(name) = v.str() {
                ed.apply_style_named(name);
            }
        });
        typed("style", glib::VariantTy::INT32, |ed, v| {
            if let Some(id) = v.get::<i32>() {
                ed.apply_paras(move |d, r| d.apply_style(r, id as usize));
            }
        });
        typed("bullet", glib::VariantTy::STRING, |ed, v| {
            if let Some(ch) = v.str().and_then(|s| s.chars().next()) {
                ed.apply_paras(move |d, r| d.set_list(r, ListKind::Bullet, ch));
            }
        });
        typed("outline-level", glib::VariantTy::INT32, |ed, v| {
            if let Some(level) = v.get::<i32>() {
                ed.apply_paras(move |d, r| d.apply_outline(r, level.clamp(0, 6) as u8));
            }
        });
    }

    pub(super) fn refresh_list_actions(&self, caret: bool) {
        for a in [
            "list-number",
            "list-bullet",
            "outline",
            "level-up",
            "level-down",
            "style-dialog",
            "style-name",
            "style",
            "bullet",
            "outline-level",
        ] {
            self.set_enabled(a, caret);
        }
    }

    /// Run `f` on the paragraphs touched by the selection (or the caret's paragraph).
    fn apply_paras(
        self: &Rc<Self>,
        f: impl FnOnce(&mut Document, Range) -> Result<(), EngineError>,
    ) {
        let Some(range) = self.para_range() else {
            return;
        };
        let result = {
            let mut st = self.state.borrow_mut();
            let Some(l) = st.loaded.as_mut() else { return };
            let before = l.doc.page_count();
            f(&mut l.doc, range).map(|_| before != l.doc.page_count())
        };
        self.finish_format(result);
    }

    /// 문단 번호 / 글머리표 / 개요 적용/해제: off when the caret's paragraph already has it.
    fn toggle_list(self: &Rc<Self>, kind: ListKind, bullet: char) {
        let current = {
            let st = self.state.borrow();
            st.caret
                .and_then(|p| st.loaded.as_ref()?.doc.list_at(p))
                .map(|l| l.kind)
        };
        if current != Some(kind) {
            self.apply_paras(move |d, r| d.set_list(r, kind, bullet));
        } else if kind == ListKind::Outline {
            // 개요 is a style: 해제 goes back to the first style (바탕글)
            self.apply_paras(|d, r| d.apply_style(r, 0));
        } else {
            self.apply_paras(|d, r| d.set_list(r, ListKind::None, bullet));
        }
    }

    fn change_level(self: &Rc<Self>, delta: i8) {
        let mut changed = false;
        let c = &mut changed;
        self.apply_paras(|d, r| d.change_level(r, delta).map(|ch| *c = ch));
        if !changed {
            self.toast("더 바꿀 수준이 없습니다");
        }
    }

    fn apply_style_named(self: &Rc<Self>, name: &str) {
        let id = {
            let st = self.state.borrow();
            st.loaded
                .as_ref()
                .and_then(|l| l.doc.styles().into_iter().find(|s| s.name == name))
                .map(|s| s.id)
        };
        match id {
            Some(id) => self.apply_paras(move |d, r| d.apply_style(r, id)),
            None => self.toast(&format!("이 문서에는 '{name}' 스타일이 없습니다")),
        }
    }

    /// 스타일 (F6): pick one of the document's styles for the caret's paragraphs.
    fn style_dialog(self: &Rc<Self>) {
        let (styles, current) = {
            let st = self.state.borrow();
            let Some(l) = st.loaded.as_ref() else { return };
            (l.doc.styles(), st.caret.and_then(|p| l.doc.style_at(p)))
        };
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .build();
        list.add_css_class("boxed-list");
        for s in &styles {
            let kind = if s.is_char { "글자" } else { "문단" };
            let row = adw::ActionRow::builder()
                .title(&s.name)
                .subtitle(kind)
                .build();
            list.append(&row);
        }
        if let Some(row) = current.and_then(|i| list.row_at_index(i as i32)) {
            list.select_row(Some(&row));
        }
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .min_content_height(320)
            .min_content_width(260)
            .build();
        let d = adw::AlertDialog::new(Some("스타일"), Some("고른 문단에 적용할 스타일"));
        d.set_extra_child(Some(&scroll));
        d.add_responses(&[("cancel", "취소"), ("ok", "설정")]);
        d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("ok"));
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        let l = list.clone();
        d.connect_response(None, move |_, r| {
            if let (Some(ed), "ok", Some(row)) = (weak.upgrade(), r, l.selected_row()) {
                let id = row.index() as usize;
                ed.apply_paras(move |d, r| d.apply_style(r, id));
            }
        });
        // double-click (or Enter) on a style applies it, like 설정
        let (dlg, weak) = (d.clone(), Rc::downgrade(self));
        list.connect_row_activated(move |_, row| {
            if let Some(ed) = weak.upgrade() {
                let id = row.index() as usize;
                ed.apply_paras(move |d, r| d.apply_style(r, id));
            }
            dlg.close();
        });
        d.present(Some(&self.win));
    }
}
