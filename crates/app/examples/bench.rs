//! Where does an edit's time go? cargo run --release -p hwpedit --example bench -- FILE
#[path = "../src/render.rs"]
mod render;
use hwpedit_engine::{Document, Hit, Range};
use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let path = std::env::args().nth(1).expect("FILE");
    let bytes = std::fs::read(&path).unwrap();
    let fmt = hwpedit_core::detect(&bytes).unwrap();
    let t = Instant::now();
    let mut d = Document::from_bytes(&bytes, fmt).unwrap();
    println!("open            {:7.1} ms", ms(t));
    let t = Instant::now();
    render::rasterize(
        "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'/>",
        1.0,
    )
    .unwrap();
    println!("fontdb load     {:7.1} ms (once)", ms(t));

    let sz = d.page_size(0).unwrap();
    let pos = (0..40)
        .flat_map(|i| (0..40).map(move |j| (i, j)))
        .find_map(|(i, j)| {
            match d.hit_test(
                0,
                sz.width * (i as f64 + 0.5) / 40.0,
                sz.height * (j as f64 + 0.5) / 40.0,
            ) {
                Ok(Hit::Editable(p)) if d.para_len(p) > 10 => Some(p),
                _ => None,
            }
        })
        .expect("editable text on page 0");
    for round in 0..3 {
        let t = Instant::now();
        let p = d.insert_text(pos, "가").unwrap();
        let t_edit = ms(t);
        let t = Instant::now();
        let svg = d.render_page_svg(0).unwrap();
        let t_svg = ms(t);
        let t = Instant::now();
        let it = render::synthesize_italics(&svg);
        let t_it = ms(t);
        let t = Instant::now();
        render::rasterize(&it, 1.0).unwrap();
        let t_r1 = ms(t);
        let t = Instant::now();
        render::rasterize(&it, 2.0).unwrap();
        let t_r2 = ms(t);
        let t = Instant::now();
        d.caret_rect(p).unwrap();
        let t_caret = ms(t);
        let t = Instant::now();
        let _ = d.char_state(p);
        let _ = d.align_at(p);
        let t_state = ms(t);
        let r = Range::new(pos.with_offset(0), pos.with_offset(d.para_len(pos))).unwrap();
        let t = Instant::now();
        let n = d.selection_rects(r).len();
        let t_sel = ms(t);
        println!(
            "edit#{round}: insert {t_edit:6.1} | svg {t_svg:6.1} ({} KB) | italic {t_it:4.1} | raster x1 {t_r1:6.1} x2 {t_r2:6.1} | caret {t_caret:5.1} | ribbon state {t_state:5.1} | sel {} chars -> {n} rects {t_sel:6.1} ms",
            svg.len() / 1024,
            d.para_len(pos)
        );
    }
}
