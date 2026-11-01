//! Typing cost as the document grows: cargo run --release -p hwpedit --example bench_long
#[path = "../src/render.rs"]
mod render;
use hwpedit_engine::{Document, Pos};
use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let blank = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/rhwp/saved/blank2010.hwp"
    ))
    .unwrap();
    let mut d = Document::from_bytes(&blank, hwpedit_core::Format::Hwp).unwrap();
    let para =
        "가나다라마바사아자차카타파하 한글 문서 편집 성능을 재기 위한 긴 문단입니다. ".repeat(4);
    let mut p = Pos::Body {
        section: 0,
        para: 0,
        offset: 0,
    };
    for target in [10usize, 100, 400, 1000] {
        while d.section_para_counts()[0] < target {
            p = d.insert_text(p, &para).unwrap();
            p = d.split_paragraph(p).unwrap();
        }
        d.break_coalescing();
        // type 5 single characters at the end, like a user would
        let mut times = [0.0f64; 7];
        for _ in 0..5 {
            let t = Instant::now();
            p = d.insert_text(p, "가").unwrap();
            times[0] += ms(t);
            let t = Instant::now();
            let r = d.caret_rect(p).unwrap();
            times[1] += ms(t);
            let t = Instant::now();
            let _ = d.char_state(p);
            let _ = d.para_state(p);
            times[2] += ms(t);
            let t = Instant::now();
            let _ = d.counts();
            times[3] += ms(t);
            let t = Instant::now();
            for pg in 0..d.page_count() {
                let _ = d.page_size(pg);
            }
            times[4] += ms(t);
            let t = Instant::now();
            let svg = d.render_page_svg(r.page).unwrap();
            times[5] += ms(t);
            let t = Instant::now();
            let _ = render::rasterize_rgba(&svg, 1.0).unwrap();
            times[6] += ms(t);
        }
        let svg = d.render_page_svg(0).unwrap();
        println!(
            "text elements on page 0: {}, svg {} KB",
            svg.matches("<text").count(),
            svg.len() / 1024
        );
        let _ = render::profile(&svg);
        let (pa, pt) = render::profile(&render::resolve_font_families(&svg));
        println!("parse+layout {pa:.1} ms, paint {pt:.1} ms");
        let n = 5.0;
        println!(
            "{target:5} paras {:3} pages | insert {:6.1} | caret {:5.1} | ribbon {:4.1} | counts {:5.1} | page sizes {:6.1} | svg {:5.1} | raster {:5.1} ms",
            d.page_count(),
            times[0] / n, times[1] / n, times[2] / n, times[3] / n, times[4] / n, times[5] / n, times[6] / n
        );
    }
}
