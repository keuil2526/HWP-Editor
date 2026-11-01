//! Render every page with the editor's own renderer as 96-dpi PPM, for comparison
//! with Hancom's PDF output. cargo run --release -p hwpedit --example render_pages -- FILE OUT_DIR
#[path = "../src/render.rs"]
mod render;

fn main() {
    let mut args = std::env::args().skip(1);
    let (file, out) = (args.next().expect("FILE"), args.next().expect("OUT_DIR"));
    let bytes = std::fs::read(&file).unwrap();
    let d = hwpedit_engine::Document::from_bytes(&bytes, hwpedit_core::detect(&bytes).unwrap())
        .unwrap();
    std::fs::create_dir_all(&out).unwrap();
    for p in 0..d.page_count() {
        let (w, h, rgba) = render::rasterize_rgba(&d.render_page_svg(p).unwrap(), 1.0).unwrap();
        // background is opaque white, so premultiplied RGB is plain RGB
        let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
        ppm.extend(rgba.chunks_exact(4).flat_map(|px| [px[0], px[1], px[2]]));
        std::fs::write(format!("{out}/page-{:03}.ppm", p + 1), ppm).unwrap();
    }
    println!("{}", d.page_count());
}
