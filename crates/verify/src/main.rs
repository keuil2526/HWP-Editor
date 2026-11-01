//! Round-trip harness. For each input file (never modified):
//!   noop   — save through hwpedit-core without edits; must be byte-identical
//!   reser  — raw rhwp re-serialization without edits (diagnostic: what we avoid)
//!   edit   — insert a marker at the start of the first non-empty body paragraph,
//!            save, reopen, and compare body text paragraph-by-paragraph
//! Writes outputs to OUT_DIR and prints one JSON line per input.
//!
//! Usage: hwpedit-verify OUT_DIR FILE...
//!        hwpedit-verify --svg FILE PAGE   (print one rendered page)
//!        hwpedit-verify --info FILE...    (JSON per file: format, pages, all text)
//!        hwpedit-verify --make-sample DIR (feature sample saved as sample.hwp and sample.hwpx)

use hwpedit_core::{save, OpenedFile, SaveOptions, SaveOutcome};
use hwpedit_engine::{CharFormat, Document, Pos, Range};
use std::path::{Path, PathBuf};

const MARKER: &str = "[편집]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "--make-sample" {
        let dir = PathBuf::from(&args[1]);
        std::fs::create_dir_all(&dir).expect("create DIR");
        for fmt in [hwpedit_core::Format::Hwp, hwpedit_core::Format::Hwpx] {
            let mut d = Document::new_blank().expect("blank");
            d.set_format(fmt);
            make_sample(&mut d).expect("sample edits");
            let bytes = d.serialize(fmt).expect("serialize").0;
            Document::validate(&bytes).expect("reparse");
            let out = dir.join(format!("sample.{}", fmt.extension()));
            std::fs::write(&out, bytes).expect("write");
            println!("{}", out.display());
        }
        return;
    }
    if args.len() >= 2 && args[0] == "--info" {
        for f in &args[1..] {
            let info = std::fs::read(f).map_err(|e| e.to_string()).and_then(|b| {
                let fmt = hwpedit_core::detect(&b).map_err(|e| e.to_string())?;
                let d = Document::from_bytes(&b, fmt).map_err(|e| e.to_string())?;
                Ok(serde_json::json!({ "file": f, "format": fmt.extension(), "pages": d.page_count(), "text": d.all_text() }))
            });
            println!(
                "{}",
                info.unwrap_or_else(|e| serde_json::json!({ "file": f, "error": e }))
            );
        }
        return;
    }
    if args.len() == 3 && args[0] == "--svg" {
        let bytes = std::fs::read(&args[1]).expect("read");
        let fmt = hwpedit_core::detect(&bytes).expect("format");
        let doc = Document::from_bytes(&bytes, fmt).expect("parse");
        print!(
            "{}",
            doc.render_page_svg(args[2].parse().expect("page"))
                .expect("render")
        );
        return;
    }
    if args.len() < 2 {
        eprintln!("usage: hwpedit-verify OUT_DIR FILE...");
        std::process::exit(2);
    }
    let out = PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).expect("create OUT_DIR");
    let opts = SaveOptions {
        backup_dir: out.join("backups"),
        ..Default::default()
    };
    for f in &args[1..] {
        let r = std::panic::catch_unwind(|| run(Path::new(f), &out, &opts))
            .unwrap_or_else(|_| serde_json::json!({ "file": f, "panic": true }));
        println!("{r}");
    }
}

fn run(input: &Path, out: &Path, opts: &SaveOptions) -> serde_json::Value {
    use serde_json::json;
    let stem = input.file_stem().unwrap().to_string_lossy().into_owned();
    let mut r = json!({ "file": input.display().to_string() });

    let opened = match OpenedFile::open(input) {
        Ok(o) => o,
        Err(e) => {
            r["open_error"] = json!(e.to_string());
            return r;
        }
    };
    let fmt = opened.format();
    let ext = fmt.extension();
    r["format"] = json!(ext);
    let t = std::time::Instant::now();
    let doc = match Document::from_bytes(opened.bytes(), fmt) {
        Ok(d) => d,
        Err(e) => {
            r["parse_error"] = json!(e.to_string());
            return r;
        }
    };
    r["parse_ms"] = json!(t.elapsed().as_millis());
    r["pages"] = json!(doc.page_count());
    let orig_text = doc.body_text();

    // noop: through the integrity layer
    let mut f = OpenedFile::open(input).unwrap();
    let target = out.join(format!("{stem}.noop.{ext}"));
    let _ = std::fs::remove_file(&target);
    r["noop"] = match save(
        &mut f,
        &target,
        fmt,
        doc.edited(),
        |_| unreachable!(),
        Document::validate,
        opts,
    ) {
        Ok(SaveOutcome::CopiedOriginal { .. }) => {
            json!({ "identical": std::fs::read(&target).unwrap() == opened.bytes() })
        }
        Ok(o) => json!({ "unexpected": format!("{o:?}") }),
        Err(e) => json!({ "error": e.to_string() }),
    };

    // reser: rhwp alone, no edits
    r["reser"] = match doc.serialize(fmt) {
        Ok(s) => {
            let p = out.join(format!("{stem}.reser.{ext}"));
            std::fs::write(&p, &s.0).unwrap();
            json!({
                "identical": s.0 == opened.bytes(),
                "size": s.0.len(),
                "reparse": Document::validate(&s.0).err(),
                "text_same": Document::from_bytes(&s.0, fmt).map(|d| d.body_text() == orig_text).ok(),
            })
        }
        Err(e) => json!({ "error": e.to_string() }),
    };

    // edit
    r["edit"] = edit(&opened, doc, &orig_text, out, &stem, opts);
    r
}

fn edit(
    opened: &OpenedFile,
    mut doc: Document,
    orig_text: &[String],
    out: &Path,
    stem: &str,
    opts: &SaveOptions,
) -> serde_json::Value {
    use serde_json::json;
    let fmt = opened.format();
    // first non-empty body paragraph (flattened index -> section/para)
    let mut flat = 0;
    let mut pos = None;
    'outer: for (si, n) in doc.section_para_counts().into_iter().enumerate() {
        for pi in 0..n {
            if !orig_text[flat].trim().is_empty() {
                pos = Some((
                    flat,
                    Pos::Body {
                        section: si,
                        para: pi,
                        offset: 0,
                    },
                ));
                break 'outer;
            }
            flat += 1;
        }
    }
    let Some((flat, pos)) = pos else {
        return json!({ "skipped": "no text" });
    };
    let after_marker = MARKER.chars().count();
    let neighbour_bold = doc.char_state(pos.with_offset(1)).map(|c| c.bold).ok();
    if let Err(e) = doc.insert_text(pos, MARKER) {
        return json!({ "insert_error": e.to_string() });
    }
    // bold the marker only
    let r = Range::new(pos, pos.with_offset(after_marker)).unwrap();
    if let Err(e) = doc.apply_char_format(
        r,
        &CharFormat {
            bold: Some(true),
            ..Default::default()
        },
    ) {
        return json!({ "format_error": e.to_string() });
    }
    let target = out.join(format!("{stem}.edit.{}", fmt.extension()));
    let _ = std::fs::copy(opened.path(), &target); // save over a copy, exercising backup
    let mut f = OpenedFile::open(&target).unwrap();
    let res = save(
        &mut f,
        &target,
        fmt,
        doc.edited(),
        |fm| doc.serialize(fm).map(|s| s.0).map_err(|e| e.to_string()),
        Document::validate,
        opts,
    );
    let backup = match res {
        Ok(SaveOutcome::Written { backup }) => backup,
        Ok(o) => return json!({ "unexpected": format!("{o:?}") }),
        Err(e) => return json!({ "save_error": e.to_string() }),
    };
    let (reopened, marker_bold, neighbour_after) =
        match Document::from_bytes(&std::fs::read(&target).unwrap(), fmt) {
            Ok(d) => (
                d.body_text(),
                d.char_state(pos.with_offset(after_marker))
                    .map(|c| c.bold)
                    .ok(),
                d.char_state(pos.with_offset(after_marker + 1))
                    .map(|c| c.bold)
                    .ok(),
            ),
            Err(e) => return json!({ "reopen_error": e.to_string() }),
        };
    let mut expected = orig_text.to_vec();
    expected[flat] = format!("{MARKER}{}", expected[flat]);
    let diffs: Vec<usize> = (0..expected.len().max(reopened.len()))
        .filter(|&i| expected.get(i) != reopened.get(i))
        .collect();
    json!({
        "para": flat,
        "backup_ok": backup.map(|b| std::fs::read(b).ok().as_deref() == Some(opened.bytes())),
        "paras_expected": expected.len(),
        "paras_reopened": reopened.len(),
        "text_mismatch_paras": diffs.iter().take(5).collect::<Vec<_>>(),
        "text_ok": diffs.is_empty(),
        "format_ok": marker_bold == Some(true) && neighbour_after == neighbour_bold,
        "losses": hwpedit_engine::loss::detect_losses(opened.bytes(), &std::fs::read(&target).unwrap(), fmt)
            .map(|l| l.iter().map(|x| x.to_string()).collect::<Vec<_>>())
            .unwrap_or_else(|e| vec![format!("검사 실패: {e}")]),
        "undo_to_unedited": undo_all(opened),
    })
}

/// Edit, format, then undo everything: the document must count as unedited again.
fn undo_all(opened: &OpenedFile) -> bool {
    let Ok(mut d) = Document::from_bytes(opened.bytes(), opened.format()) else {
        return false;
    };
    let p = Pos::Body {
        section: 0,
        para: 0,
        offset: 0,
    };
    let Ok(q) = d.insert_text(p, "ab") else {
        return false;
    };
    if d.split_paragraph(q).is_err() {
        return false;
    }
    while d.can_undo() {
        if d.undo(p).is_err() {
            return false;
        }
    }
    !d.edited()
}

/// Exercise the editor's features: char/para formatting, a merged table, a page break.
fn make_sample(d: &mut Document) -> Result<(), hwpedit_engine::EngineError> {
    use hwpedit_engine::{Align, ParaFormat};
    let p0 = Pos::Body {
        section: 0,
        para: 0,
        offset: 0,
    };
    let title = "hwpedit 기능 견본";
    let p = d.insert_text(p0, title)?;
    let r = Range::new(p0, p).unwrap();
    d.apply_char_format(
        r,
        &CharFormat {
            bold: Some(true),
            size_pt: Some(18.0),
            color: Some("#1f4e9e".into()),
            ..Default::default()
        },
    )?;
    d.set_align(r, Align::Center)?;
    let p = d.split_paragraph(p)?;
    let line = "진하게 기울임 밑줄 취소선 보통";
    let e = d.insert_text(p, line)?;
    for (a, b, f) in [
        (
            0,
            3,
            CharFormat {
                bold: Some(true),
                ..Default::default()
            },
        ),
        (
            4,
            7,
            CharFormat {
                italic: Some(true),
                ..Default::default()
            },
        ),
        (
            8,
            10,
            CharFormat {
                underline: Some(true),
                ..Default::default()
            },
        ),
        (
            11,
            14,
            CharFormat {
                strike: Some(true),
                ..Default::default()
            },
        ),
    ] {
        d.apply_char_format(Range::new(p.with_offset(a), p.with_offset(b)).unwrap(), &f)?;
    }
    let p = d.split_paragraph(e)?;
    let e = d.insert_text(p, "줄 간격 200%, 왼쪽 여백 20pt, 첫 줄 들여쓰기 10pt인 문단입니다. 한 줄을 넘기도록 충분히 긴 문장을 씁니다. 한 줄을 넘기도록 충분히 긴 문장을 씁니다.")?;
    d.set_para_format(
        Range { start: p, end: p },
        &ParaFormat {
            line_spacing_pct: Some(200),
            margin_left_pt: Some(20.0),
            indent_pt: Some(10.0),
            ..Default::default()
        },
    )?;
    let p = d.split_paragraph(e)?;
    let c = d.create_table(p, 3, 3)?;
    let mut cell = c;
    for (i, t) in ["가", "나", "다", "라", "마", "바", "사", "아", "자"]
        .iter()
        .enumerate()
    {
        if let Some(x) = d.cell_at(c, (i / 3) as u16, (i % 3) as u16) {
            cell = d.insert_text(x, t)?;
        }
    }
    let _ = cell;
    // table sizing: move the first column border, make row 1 taller, Tab past the last cell
    let first = d.cell_at(c, 0, 0).unwrap();
    d.resize_table(first, false, 8, hwpedit_engine::Resize::Border)?;
    d.resize_table(first, true, 6, hwpedit_engine::Resize::Table)?;
    let last = d.cell_at(c, 2, 2).unwrap();
    let added = d.tab_forward(last)?;
    d.insert_text(added, "Tab으로 추가한 줄")?;
    let a = d.cell_at(c, 0, 0).unwrap();
    let b = d.cell_at(c, 0, 1).unwrap();
    d.merge_cells(a, b)?;
    let Pos::Cell {
        section,
        parent_para,
        ..
    } = c
    else {
        unreachable!()
    };
    let after = Pos::Body {
        section,
        para: parent_para + 1,
        offset: 0,
    };
    let after = if d.para_len(after) == 0 && d.caret_rect(after).is_ok() {
        after
    } else {
        Pos::Body {
            section,
            para: parent_para,
            offset: d.para_len(Pos::Body {
                section,
                para: parent_para,
                offset: 0,
            }),
        }
    };
    let e = d.insert_text(after, "표 아래 문단")?;
    let q = d.insert_page_break(e)?;
    let q = d.insert_text(q, "둘째 쪽: 모서리로 크기를 바꾼 표")?;
    let q = d.split_paragraph(q)?;
    let t2 = d.create_table(q, 2, 2)?;
    let t2 = d.insert_text(t2, "0.6배 너비 · 1.5배 높이")?;
    d.scale_table(t2, 0.6, 1.5)?;
    Ok(())
}
