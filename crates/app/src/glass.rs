//! Liquid-glass panes: a container that paints a blurred copy of the widget
//! behind it (the document canvas), a translucent tint, a specular top edge and
//! a hairline rim, then its children. GTK CSS has no backdrop-filter, so the
//! blur is done with a render-node blur over a live `WidgetPaintable` of the
//! canvas. The pane must not be inside the source widget (it would recurse).

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{graphene, gsk};

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    pub struct GlassPane {
        pub source: RefCell<Option<gtk::Widget>>,
        pub paintable: RefCell<Option<gtk::WidgetPaintable>>,
        pub radius: Cell<f32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GlassPane {
        const NAME: &'static str = "HwpeditGlassPane";
        type Type = super::GlassPane;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_layout_manager_type::<gtk::BinLayout>();
            klass.set_css_name("glass");
        }
    }

    impl ObjectImpl for GlassPane {
        fn dispose(&self) {
            while let Some(c) = self.obj().first_child() {
                c.unparent();
            }
        }
    }

    impl WidgetImpl for GlassPane {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let r = self.radius.get();
            let bounds = graphene::Rect::new(0.0, 0.0, w, h);
            let rounded = gsk::RoundedRect::from_rect(bounds, r);
            let dark = adw::StyleManager::default().is_dark();

            snapshot.push_rounded_clip(&rounded);
            // 1. the canvas behind us, blurred
            if let (Some(src), Some(p)) = (
                self.source.borrow().as_ref(),
                self.paintable.borrow().as_ref(),
            ) {
                if let Some(at) = obj.compute_point(src, &graphene::Point::new(0.0, 0.0)) {
                    snapshot.push_blur(22.0);
                    snapshot.save();
                    snapshot.translate(&graphene::Point::new(-at.x(), -at.y()));
                    p.snapshot(snapshot, src.width() as f64, src.height() as f64);
                    snapshot.restore();
                    snapshot.pop();
                }
            }
            // 2. tint: light frosted glass, darker smoked glass in dark mode
            let tint = if dark {
                gtk::gdk::RGBA::new(0.13, 0.14, 0.17, 0.58)
            } else {
                gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 0.56)
            };
            snapshot.append_color(&tint, &bounds);
            // 3. a soft sheen from the top edge
            let sheen = graphene::Rect::new(0.0, 0.0, w, (h * 0.55).min(46.0));
            let top = gtk::gdk::RGBA::new(1.0, 1.0, 1.0, if dark { 0.10 } else { 0.38 });
            let clear = gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 0.0);
            snapshot.append_linear_gradient(
                &sheen,
                &graphene::Point::new(0.0, 0.0),
                &graphene::Point::new(0.0, sheen.height()),
                &[
                    gsk::ColorStop::new(0.0, top),
                    gsk::ColorStop::new(1.0, clear),
                ],
            );
            // 4. specular rim: bright top edge, faint everywhere else
            snapshot.append_inset_shadow(
                &rounded,
                &gtk::gdk::RGBA::new(1.0, 1.0, 1.0, if dark { 0.22 } else { 0.85 }),
                0.0,
                1.0,
                0.0,
                0.5,
            );
            let rim = gtk::gdk::RGBA::new(1.0, 1.0, 1.0, if dark { 0.14 } else { 0.55 });
            snapshot.append_border(&rounded, &[1.0; 4], &[rim; 4]);
            snapshot.pop();

            self.parent_snapshot(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct GlassPane(ObjectSubclass<imp::GlassPane>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl GlassPane {
    /// A glass pane with `child`, showing `source` (blurred) through it.
    pub fn new(
        child: &impl IsA<gtk::Widget>,
        source: &impl IsA<gtk::Widget>,
        radius: f32,
    ) -> GlassPane {
        let pane: GlassPane = glib::Object::new();
        child.set_parent(&pane);
        let imp = pane.imp();
        imp.radius.set(radius);
        let paintable = gtk::WidgetPaintable::new(Some(source));
        let weak = pane.downgrade();
        paintable.connect_invalidate_contents(move |_| {
            if let Some(p) = weak.upgrade() {
                p.queue_draw();
            }
        });
        *imp.source.borrow_mut() = Some(source.clone().upcast());
        *imp.paintable.borrow_mut() = Some(paintable);
        pane
    }
}
