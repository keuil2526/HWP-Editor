//! Page rasterization: rhwp SVG -> resvg -> cairo image surface.

use gtk::cairo;
use resvg::{tiny_skia, usvg};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

fn fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        // Without this, every text lookup re-reads the font file from disk:
        // a page with ~1000 glyph runs took ~170 ms instead of ~20 ms.
        let ids: Vec<_> = db.faces().map(|f| f.id).collect();
        for id in ids {
            // SAFETY: system font files are not modified while the editor runs.
            unsafe {
                db.make_shared_face_data(id);
            }
        }
        // rhwp's family lists end in generic names; map them to installed Korean faces.
        db.set_sans_serif_family("Malgun Gothic");
        db.set_serif_family("NanumMyeongjo");
        Arc::new(db)
    })
    .clone()
}

/// Installed family names, lower-cased.
fn installed_families() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| {
        fontdb()
            .faces()
            .flat_map(|f| f.families.iter().map(|(n, _)| n.to_lowercase()))
            .collect()
    })
}

/// The first installed family of a CSS family list (or the generic it ends with).
fn resolve_family(list: &str) -> String {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(r) = cache.lock().unwrap().get(list) {
        return r.clone();
    }
    let installed = installed_families();
    let names: Vec<String> = list
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .split(',')
        .map(|n| n.trim().trim_matches(|c| c == '\'' || c == '"').to_owned())
        .filter(|n| !n.is_empty())
        .collect();
    let pick = names
        .iter()
        .find(|n| installed.contains(&n.to_lowercase()))
        .map(|n| format!("&apos;{n}&apos;"))
        .or_else(|| {
            names
                .iter()
                .rev()
                .find(|n| matches!(n.as_str(), "serif" | "sans-serif" | "monospace"))
                .cloned()
        })
        .unwrap_or_else(|| list.to_owned());
    cache.lock().unwrap().insert(list.to_owned(), pick.clone());
    pick
}

/// rhwp writes a long fallback list on every glyph run (함초롬바탕, 바탕, ...). When the
/// first names are not installed, usvg searched ~1000 faces per name for every run:
/// a dense page took ~150 ms to lay out. Resolve each distinct list once instead;
/// the face drawn is the same one usvg would have chosen.
pub fn resolve_font_families(svg: &str) -> String {
    const KEY: &str = "font-family=\"";
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(i) = rest.find(KEY) {
        out.push_str(&rest[..i + KEY.len()]);
        rest = &rest[i + KEY.len()..];
        let Some(j) = rest.find('"') else { break };
        out.push_str(&resolve_family(&rest[..j]));
        rest = &rest[j..];
    }
    out.push_str(rest);
    out
}

/// Installed families (lower-cased) that have a real italic face.
fn italic_families() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| {
        fontdb()
            .faces()
            .filter(|f| f.style == usvg::fontdb::Style::Italic)
            .flat_map(|f| f.families.iter().map(|(n, _)| n.to_lowercase()))
            .collect()
    })
}

/// Whether a `<text>` tag names (after `resolve_font_families`) a family with a real italic.
fn has_real_italic(tag: &str) -> bool {
    const KEY: &str = "font-family=\"";
    let Some(i) = tag.find(KEY) else { return false };
    let list = &tag[i + KEY.len()..];
    let family = list[..list.find('"').unwrap_or(list.len())]
        .replace("&apos;", "")
        .to_lowercase();
    italic_families().contains(family.trim())
}

/// Slant of synthetic italics (tan ~12°).
const OBLIQUE: f64 = 0.21;

/// Korean faces have no italic, and resvg does not synthesize one, so italic
/// `<text x= y=>` glyphs are drawn upright and skewed about their baseline.
/// A family with a real italic face (수식's STIX Two Text) keeps it: rhwp places 수식
/// scripts by that face's advances, and the skewed upright capitals lean ~0.14em further
/// right, over a following superscript.
pub fn synthesize_italics(svg: &str) -> String {
    const ITALIC: &str = " font-style=\"italic\"";
    if !svg.contains(ITALIC) {
        return svg.to_owned();
    }
    let mut out = String::with_capacity(svg.len() + svg.len() / 16);
    let mut rest = svg;
    while let Some(start) = rest.find("<text ") {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find('>').map_or(rest.len(), |e| e + 1);
        let tag = &rest[..end];
        match (
            tag.contains(ITALIC) && !tag.contains("transform=") && !has_real_italic(tag),
            attr(tag, "y"),
        ) {
            (true, Some(y)) => {
                let tag = tag.replacen(ITALIC, "", 1);
                let at = "<text ".len();
                out.push_str(&tag[..at]);
                out.push_str(&format!(
                    "transform=\"matrix(1 0 {} 1 {} 0)\" ",
                    -OBLIQUE,
                    OBLIQUE * y
                ));
                out.push_str(&tag[at..]);
            }
            _ => out.push_str(tag),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Width and height of an SVG's root element (px), e.g. a 수식 preview.
#[allow(dead_code)] // unused by the render_pages example, which includes this file
pub fn svg_size(svg: &str) -> Option<(f64, f64)> {
    let start = svg.find("<svg")?;
    let tag = &svg[start..start + svg[start..].find('>')?];
    Some((attr(tag, "width")?, attr(tag, "height")?))
}

fn attr(tag: &str, name: &str) -> Option<f64> {
    let key = format!(" {name}=\"");
    let i = tag.find(&key)? + key.len();
    tag[i..].split('"').next()?.parse().ok()
}

/// Rasterize `svg` on white at `scale` device pixels per page pixel.
/// Returns width, height and premultiplied RGBA bytes.
pub fn rasterize_rgba(svg: &str, scale: f64) -> Result<(u32, u32, Vec<u8>), String> {
    let opts = usvg::Options {
        fontdb: fontdb(),
        font_family: "Malgun Gothic".into(),
        ..Default::default()
    };
    let svg = synthesize_italics(&resolve_font_families(svg));
    let tree = usvg::Tree::from_str(&svg, &opts).map_err(|e| e.to_string())?;
    let size = tree.size();
    let w = (size.width() as f64 * scale).ceil().max(1.0) as u32;
    let h = (size.height() as f64 * scale).ceil().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("page too large")?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let t = tiny_skia::Transform::from_scale(scale as f32, scale as f32);
    resvg::render(&tree, t, &mut pixmap.as_mut());
    Ok((w, h, pixmap.take()))
}

/// One drawing element of rhwp's SVG: a `<text>…</text>` run or a single tag, and
/// whether it sits in a `<g transform>` or nested `<svg>` (수식, 차트), where its
/// coordinates are not page coordinates.
fn elements(svg: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut moved: Vec<bool> = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        let end = if rest.starts_with("<text") {
            rest.find("</text>").map(|e| e + "</text>".len())
        } else {
            rest.find('>').map(|e| e + 1)
        };
        let Some(end) = end else { break };
        let e = &rest[..end];
        let inside = moved.last().copied().unwrap_or(false);
        out.push((e, inside));
        if e.starts_with("</g") || e.starts_with("</svg") {
            moved.pop();
        } else if (e.starts_with("<g") || e.starts_with("<svg")) && !e.ends_with("/>") {
            let nested_svg = e.starts_with("<svg") && !moved.is_empty();
            moved.push(inside || nested_svg || e.contains("transform="));
        }
        rest = &rest[end..];
    }
    out
}

/// rhwp numbers clip paths with a counter, so one more table cell renumbers every
/// clip after it. For comparing two renders, name each id by its order of definition.
fn canonical_ids<'a>(els: &[&'a str]) -> Vec<std::borrow::Cow<'a, str>> {
    let mut order: HashMap<&str, usize> = HashMap::new();
    for e in els {
        if let Some(i) = e.find(" id=\"") {
            let id = &e[i + 5..];
            if let Some(j) = id.find('"') {
                let n = order.len();
                order.entry(&id[..j]).or_insert(n);
            }
        }
    }
    els.iter()
        .map(|e| {
            if !e.contains(" id=\"") && !e.contains("url(#") {
                return std::borrow::Cow::Borrowed(*e);
            }
            let mut out = String::with_capacity(e.len());
            let mut rest = *e;
            loop {
                let next = [" id=\"", "url(#"]
                    .iter()
                    .filter_map(|k| rest.find(k).map(|i| (i, k.len())))
                    .min();
                let Some((i, k)) = next else { break };
                out.push_str(&rest[..i + k]);
                rest = &rest[i + k..];
                let j = rest.find(['"', ')']).unwrap_or(rest.len());
                match order.get(&rest[..j]) {
                    Some(n) => out.push_str(&format!("#{n}")),
                    None => out.push_str(&rest[..j]),
                }
                rest = &rest[j..];
            }
            out.push_str(rest);
            std::borrow::Cow::Owned(out)
        })
        .collect()
}

fn keyed<'a>(els: &[(&'a str, bool)]) -> Vec<(std::borrow::Cow<'a, str>, bool)> {
    let names: Vec<&str> = els.iter().map(|e| e.0).collect();
    canonical_ids(&names)
        .into_iter()
        .zip(els.iter().map(|e| e.1))
        .collect()
}

/// The vertical span (page px) an element can paint, if it is one we can bound.
fn extent(el: &str) -> Option<(f64, f64)> {
    let tag = &el[..el.find('>')? + 1];
    let stroke = attr(tag, "stroke-width").unwrap_or(1.0);
    if tag.starts_with("<text") {
        if tag.contains("transform=") || tag.contains("rotate=") || tag.contains(" dy=\"") {
            return None;
        }
        let (y, size) = (attr(tag, "y")?, attr(tag, "font-size").unwrap_or(16.0));
        Some((y - 1.3 * size, y + 0.6 * size))
    } else if tag.starts_with("<line") {
        let (a, b) = (attr(tag, "y1")?, attr(tag, "y2")?);
        Some((a.min(b) - stroke, a.max(b) + stroke))
    } else if tag.starts_with("<rect") || tag.starts_with("<image") {
        if tag.contains("transform=") {
            return None;
        }
        let (y, h) = (attr(tag, "y").unwrap_or(0.0), attr(tag, "height")?);
        Some((y - stroke, y + h + stroke))
    } else {
        None
    }
}

/// A strip of the page redrawn after a small edit, in cairo's byte order: rows
/// `y`..`y + h` of the full page raster at the same scale.
#[allow(dead_code)] // fields unused by the examples, which include this file
pub struct Band {
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// Redraw only the part of the page an edit changed (typing, deleting on a line),
/// instead of the whole page: laying out every glyph run of a dense page takes
/// ~120 ms, the few runs of one line ~1 ms. The strip covers every element that
/// differs between `old` and `new`; all other elements reaching into it are drawn
/// too, so its pixels are those of a full render. `None` when the change is too big
/// or not one we can bound (the whole page is drawn then).
#[allow(dead_code)] // unused by the render_pages example, which includes this file
pub fn rasterize_band(old: &str, new: &str, scale: f64) -> Option<Band> {
    let (old_els, new_els) = (elements(old), elements(new));
    // an element is keyed with whether it is in a moved group: a change in there
    // cannot be placed on the page
    let mut count: HashMap<(std::borrow::Cow<str>, bool), i32> = HashMap::new();
    for k in keyed(&old_els) {
        *count.entry(k).or_default() += 1;
    }
    for k in keyed(&new_els) {
        *count.entry(k).or_default() -= 1;
    }
    let (mut y0, mut y1, mut changed) = (f64::MAX, f64::MIN, 0);
    for ((e, moved), n) in count {
        if n != 0 {
            if moved {
                return None;
            }
            let (a, b) = extent(&e)?;
            y0 = y0.min(a);
            y1 = y1.max(b);
            changed += 1;
        }
    }
    if changed == 0 {
        return None;
    }
    let (page_w, page_h) = svg_size(new)?;
    if changed > 600 || y1 - y0 > page_h * 0.5 {
        return None;
    }
    let py0 = ((y0 - 2.0) * scale).floor().max(0.0);
    let py1 = ((y1 + 2.0) * scale).ceil().min((page_h * scale).ceil());
    if py1 <= py0 {
        return None;
    }
    // what reaches into the strip, in page px
    let (b0, b1) = (py0 / scale, py1 / scale);
    let mut strip = String::with_capacity(new.len() / 4);
    let mut rest = new;
    for &(e, moved) in &new_els {
        // keep whatever sits between elements (text inside <clipPath> etc. is none)
        let at = rest.find(e).unwrap_or(0);
        strip.push_str(&rest[..at]);
        rest = &rest[at + e.len()..];
        // in a moved group the coordinates are not the page's: always draw it
        let skip = !moved
            && (e.starts_with("<text") || e.starts_with("<image"))
            && extent(e).is_some_and(|(a, b)| b < b0 || a > b1);
        if !skip {
            strip.push_str(e);
        }
    }
    strip.push_str(rest);

    let opts = usvg::Options {
        fontdb: fontdb(),
        font_family: "Malgun Gothic".into(),
        ..Default::default()
    };
    let svg = synthesize_italics(&resolve_font_families(&strip));
    let tree = usvg::Tree::from_str(&svg, &opts).ok()?;
    let w = (page_w * scale).ceil().max(1.0) as u32;
    let h = (py1 - py0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let t = tiny_skia::Transform::from_row(scale as f32, 0.0, 0.0, scale as f32, 0.0, -py0 as f32);
    resvg::render(&tree, t, &mut pixmap.as_mut());
    let mut data = pixmap.take();
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Some(Band {
        y: py0 as u32,
        w,
        h,
        data,
    })
}

/// Rasterize into cairo's byte order (premultiplied BGRA on little-endian).
/// Thread-safe: this is what the worker threads run.
#[allow(dead_code)] // unused by the render_pages example, which includes this file
pub fn rasterize_bgra(svg: &str, scale: f64) -> Result<(u32, u32, Vec<u8>), String> {
    let (w, h, mut data) = rasterize_rgba(svg, scale)?;
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Ok((w, h, data))
}

/// 보기 › 회색조: BGRA pixels to their luminance, in place. Screen only.
#[allow(dead_code)] // unused by the examples, which include this file
pub fn grayscale(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        // Rec. 601 weights for BGRA (0.114 B, 0.587 G, 0.299 R)
        let y = (px[0] as u32 * 29 + px[1] as u32 * 150 + px[2] as u32 * 77) >> 8;
        px[0] = y as u8;
        px[1] = y as u8;
        px[2] = y as u8;
    }
}

/// Wrap BGRA bytes as a cairo surface (main thread).
#[allow(dead_code)]
pub fn surface_from_bgra(w: u32, h: u32, data: Vec<u8>) -> Result<cairo::ImageSurface, String> {
    let stride = cairo::Format::ARgb32
        .stride_for_width(w)
        .map_err(|e| e.to_string())?;
    debug_assert_eq!(stride as u32, w * 4);
    cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, w as i32, h as i32, stride)
        .map_err(|e| e.to_string())
}

/// Rasterize `svg` at `scale` device pixels per page pixel (synchronously, for printing).
#[allow(dead_code)] // unused by the render_pages example, which includes this file
pub fn rasterize(svg: &str, scale: f64) -> Result<cairo::ImageSurface, String> {
    let (w, h, data) = rasterize_bgra(svg, scale)?;
    surface_from_bgra(w, h, data)
}

/// Parse and paint timings for one page (diagnostics).
#[allow(dead_code)]
pub fn profile(svg: &str) -> (f64, f64) {
    let opts = usvg::Options {
        fontdb: fontdb(),
        font_family: "Malgun Gothic".into(),
        ..Default::default()
    };
    let t = std::time::Instant::now();
    let tree = usvg::Tree::from_str(svg, &opts).unwrap();
    let parse = t.elapsed().as_secs_f64() * 1000.0;
    let s = tree.size();
    let mut pm = tiny_skia::Pixmap::new(s.width() as u32, s.height() as u32).unwrap();
    let t = std::time::Instant::now();
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pm.as_mut());
    (parse, t.elapsed().as_secs_f64() * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn italic_glyphs_get_baseline_skew() {
        let svg = r##"<svg><text x="10" y="100" font-size="12" font-style="italic" fill="#000">가</text><text x="20" y="100">나</text></svg>"##;
        let out = synthesize_italics(svg);
        assert!(out.contains(r##"<text transform="matrix(1 0 -0.21 1 21 0)" x="10" y="100" font-size="12" fill="#000">가</text>"##), "{out}");
        assert!(out.contains(r#"<text x="20" y="100">나</text>"#));
        assert!(!out.contains("italic"));
    }

    #[test]
    fn font_lists_resolve_to_one_installed_family_or_generic() {
        let svg = r#"<text font-family="&apos;No Such Font 1&apos;,&apos;No Such Font 2&apos;,serif">a</text>"#;
        assert_eq!(
            resolve_font_families(svg),
            r#"<text font-family="serif">a</text>"#
        );
        let other = r#"<rect fill="red"/>"#;
        assert_eq!(resolve_font_families(other), other);
    }

    fn page(lines: &[&str]) -> String {
        let mut s = String::from(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="300" viewBox="0 0 200 300">
<defs>
<clipPath id="body-clip-3"><rect x="10" y="10" width="180" height="280"/></clipPath>
</defs>
<rect x="0" y="0" width="200" height="300" fill="#ffffff"/>
<g clip-path="url(#body-clip-3)">"##,
        );
        for (i, l) in lines.iter().enumerate() {
            let y = 40 + 30 * i;
            for (k, c) in l.chars().enumerate() {
                s.push_str(&format!(
                    r##"<text x="{}" y="{y}" font-size="16" fill="#000000">{c}</text>"##,
                    20 + 16 * k
                ));
            }
        }
        s.push_str(
            r##"<line x1="20" y1="200" x2="180" y2="200" stroke="#000000" stroke-width="1"/>"##,
        );
        s.push_str("</g>\n</svg>\n");
        s
    }

    #[test]
    fn typed_line_is_redrawn_exactly_as_in_a_full_render() {
        let old = page(&["가나다", "라마바", "사아자"]);
        let new = page(&["가나다", "라마X바", "사아자"]);
        let scale = 1.5;
        let band = rasterize_band(&old, &new, scale).expect("one line changed");
        let (w, h, full) = rasterize_bgra(&new, scale).unwrap();
        assert_eq!(band.w, w);
        // only around the second line (baseline 70), not the whole page
        assert!(band.y as f64 > 30.0 * scale && ((band.y + band.h) as f64) < 100.0 * scale);
        assert!(band.h < h / 3);
        let row = (w * 4) as usize;
        let at = band.y as usize * row;
        assert!(full[at..at + band.data.len()] == band.data[..]);
    }

    #[test]
    fn unchanged_or_unbounded_edits_draw_the_whole_page() {
        let a = page(&["가나다"]);
        assert!(rasterize_band(&a, &a, 1.0).is_none());
        let b = a.replace("<g clip-path", "<g transform=\"rotate(3)\" clip-path");
        assert!(rasterize_band(&a, &b, 1.0).is_none());
    }

    #[test]
    fn equation_glyphs_in_a_moved_group_stay_in_the_strip() {
        // a 수식 is drawn in its own coordinates under translate(): its glyph at y=13
        // is on the second line of the page, not at its top
        let eq = r##"<g transform="translate(120,60)"><text x="2" y="13" font-size="13" fill="#000000">x</text></g>"##;
        let old = page(&["가나다", "라마바"]).replace("</g>\n</svg>", &format!("{eq}</g>\n</svg>"));
        let new =
            page(&["가나다", "라마X바"]).replace("</g>\n</svg>", &format!("{eq}</g>\n</svg>"));
        let scale = 1.0;
        let band = rasterize_band(&old, &new, scale).expect("one line changed");
        let (w, _, full) = rasterize_bgra(&new, scale).unwrap();
        let at = band.y as usize * (w * 4) as usize;
        assert!(full[at..at + band.data.len()] == band.data[..]);
        // changing the equation itself cannot be placed: the whole page is drawn
        let changed = new.replace(">x</text></g>", ">y</text></g>");
        assert!(rasterize_band(&new, &changed, scale).is_none());
    }

    #[test]
    fn renumbered_clip_paths_are_not_a_change() {
        let a = page(&["가나다", "라마바"]);
        let b = page(&["가나다", "라마X바"]).replace("body-clip-3", "body-clip-7");
        let band = rasterize_band(&a, &b, 1.0).expect("only the second line changed");
        assert!(band.h < 60);
    }

    #[test]
    fn real_italic_faces_are_not_skewed() {
        let Some(family) = italic_families().iter().next() else {
            panic!("no installed font has an italic face");
        };
        let svg = format!(
            r#"<svg><text x="1" y="2" font-style="italic" font-family="&apos;{family}&apos;">m</text></svg>"#
        );
        assert_eq!(synthesize_italics(&svg), svg);
    }

    #[test]
    fn non_italic_svg_is_unchanged() {
        let svg = r#"<svg><text x="1" y="2">a</text></svg>"#;
        assert_eq!(synthesize_italics(svg), svg);
    }
}
