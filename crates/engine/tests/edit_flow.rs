//! The edit sequence the GTK window performs, run headless on a Hancom-made
//! blank document shipped with rhwp.

use hwpedit_core::{detect, Format};
use hwpedit_engine::{Document, Hit, Pos};

fn blank() -> Vec<u8> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/rhwp/saved/blank2010.hwp"
    );
    std::fs::read(p).expect("vendor/rhwp/saved/blank2010.hwp (clone rhwp into vendor/)")
}

fn open(bytes: &[u8]) -> Document {
    Document::from_bytes(bytes, detect(bytes).unwrap()).unwrap()
}

const START: Pos = Pos::Body {
    section: 0,
    para: 0,
    offset: 0,
};

#[test]
fn opening_does_not_mark_edited() {
    let d = open(&blank());
    assert!(!d.edited());
    assert!(d.page_count() >= 1);
}

#[test]
fn type_split_merge_delete() {
    let mut d = open(&blank());
    let p = d.insert_text(START, "안녕하세요 HWP").unwrap();
    assert_eq!(p.offset(), "안녕하세요 HWP".chars().count());
    assert!(d.edited());

    // Enter in the middle, then Backspace at the start of the new paragraph.
    let second = d.split_paragraph(START.with_offset(5)).unwrap();
    assert_eq!(
        second,
        Pos::Body {
            section: 0,
            para: 1,
            offset: 0
        }
    );
    assert_eq!(&d.body_text()[..2], ["안녕하세요", " HWP"]);
    let joined = d.merge_with_previous(second).unwrap().unwrap();
    assert_eq!(joined, START.with_offset(5));
    assert_eq!(d.body_text()[0], "안녕하세요 HWP");
    assert_eq!(
        d.merge_with_previous(START).unwrap(),
        None,
        "first paragraph has nothing to join"
    );

    // Backspace one char, Delete one char.
    d.delete_text(START.with_offset(4), 1).unwrap();
    d.delete_text(START.with_offset(0), 1).unwrap();
    assert_eq!(d.body_text()[0], "녕하세 HWP");
    assert_eq!(d.para_len(START), "녕하세 HWP".chars().count());
}

#[test]
fn click_at_caret_position_hits_same_place() {
    let mut d = open(&blank());
    d.insert_text(START, "가나다라마바사").unwrap();
    let pos = START.with_offset(3);
    let r = d.caret_rect(pos).unwrap();
    match d.hit_test(r.page, r.x + 0.5, r.y + r.height / 2.0).unwrap() {
        Hit::Editable(p) => assert_eq!(p, pos),
        other => panic!("{other:?}"),
    }
}

#[test]
fn edited_save_reopens_with_same_text_in_both_formats() {
    for fmt in [Format::Hwp, Format::Hwpx] {
        let mut d = open(&blank());
        d.insert_text(START, "저장 검증").unwrap();
        let p = d.split_paragraph(START.with_offset(2)).unwrap();
        d.insert_text(p, "둘째 줄 ").unwrap();
        let bytes = d.serialize(fmt).unwrap().0;
        assert_eq!(detect(&bytes), Ok(fmt));
        Document::validate(&bytes).unwrap();
        let back = Document::from_bytes(&bytes, fmt).unwrap();
        assert_eq!(&back.body_text()[..2], ["저장", "둘째 줄  검증"], "{fmt:?}");
    }
}

#[test]
fn every_page_renders() {
    let d = open(&blank());
    for p in 0..d.page_count() {
        let svg = d.render_page_svg(p).unwrap();
        assert!(svg.starts_with("<svg"));
        let sz = d.page_size(p).unwrap();
        assert!(sz.width > 100.0 && sz.height > 100.0);
    }
}

use hwpedit_engine::{Align, CharFormat, Range};

#[test]
fn typing_coalesces_into_one_undo_step_and_undo_to_saved_state_is_unedited() {
    let mut d = open(&blank());
    let mut p = START;
    for ch in ["가", "나", "다"] {
        p = d.insert_text(p, ch).unwrap();
    }
    d.break_coalescing();
    let q = d.split_paragraph(p).unwrap();
    d.insert_text(q, "X").unwrap();
    assert_eq!(&d.body_text()[..2], ["가나다", "X"]);

    // undo "X", undo split, undo the whole typing run
    d.undo(q.with_offset(1)).unwrap();
    assert_eq!(&d.body_text()[..2], ["가나다", ""]);
    d.undo(q).unwrap();
    assert_eq!(d.body_text()[0], "가나다");
    let caret = d.undo(p).unwrap();
    assert_eq!(caret, Some(START));
    assert_eq!(d.body_text()[0], "");
    assert!(!d.edited(), "back at the opened state");
    assert!(!d.can_undo());

    // redo everything
    d.redo(START).unwrap();
    assert_eq!(d.body_text()[0], "가나다");
    assert!(d.edited());
    d.redo(p).unwrap();
    d.redo(q).unwrap();
    assert_eq!(&d.body_text()[..2], ["가나다", "X"]);
    assert!(!d.can_redo());
}

#[test]
fn new_edit_clears_redo() {
    let mut d = open(&blank());
    let p = d.insert_text(START, "abc").unwrap();
    d.undo(p).unwrap();
    assert!(d.can_redo());
    d.insert_text(START, "z").unwrap();
    assert!(!d.can_redo());
}

#[test]
fn char_format_applies_to_range_and_survives_save() {
    for fmt in [Format::Hwp, Format::Hwpx] {
        let mut d = open(&blank());
        d.insert_text(START, "굵게 보통").unwrap();
        let r = Range::new(START, START.with_offset(2)).unwrap();
        let f = CharFormat {
            bold: Some(true),
            size_pt: Some(16.0),
            color: Some("#ff0000".into()),
            ..Default::default()
        };
        d.apply_char_format(r, &f).unwrap();
        let s = d.char_state(START.with_offset(2)).unwrap();
        assert!(s.bold);
        assert_eq!(s.size_pt, 16.0);
        assert_eq!(s.color.to_lowercase(), "#ff0000");
        assert!(
            !d.char_state(START.with_offset(5)).unwrap().bold,
            "outside the range is untouched"
        );

        let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
        let s = back.char_state(START.with_offset(2)).unwrap();
        assert!(s.bold && s.size_pt == 16.0, "{fmt:?}: {s:?}");
        assert!(
            !back.char_state(START.with_offset(5)).unwrap().bold,
            "{fmt:?}"
        );

        // one undo step reverts the formatting
        d.undo(START).unwrap();
        assert!(!d.char_state(START.with_offset(2)).unwrap().bold);
    }
}

#[test]
fn font_change_registers_face() {
    let mut d = open(&blank());
    d.insert_text(START, "글꼴").unwrap();
    let r = Range::new(START, START.with_offset(2)).unwrap();
    d.apply_char_format(
        r,
        &CharFormat {
            font: Some("NanumGothic".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        d.char_state(START.with_offset(1)).unwrap().font,
        "NanumGothic"
    );
    let bytes = d.serialize(Format::Hwp).unwrap().0;
    let back = Document::from_bytes(&bytes, Format::Hwp).unwrap();
    assert_eq!(
        back.char_state(START.with_offset(1)).unwrap().font,
        "NanumGothic"
    );
}

#[test]
fn alignment_and_replace_range() {
    let mut d = open(&blank());
    let p = d.insert_text(START, "첫째").unwrap();
    let q = d.split_paragraph(p).unwrap();
    d.insert_text(q, "둘째").unwrap();
    let all = d.container_range(START);
    assert_eq!(all.end, q.with_offset(2));
    d.set_align(all, Align::Center).unwrap();
    assert_eq!(d.align_at(START), Some(Align::Center));
    assert_eq!(d.align_at(q), Some(Align::Center));

    // replace across the paragraph break
    let r = Range::new(START.with_offset(1), q.with_offset(1)).unwrap();
    let c = d.replace_range(r, "-").unwrap();
    assert_eq!(c, START.with_offset(2));
    assert_eq!(d.body_text()[0], "첫-째");
    assert_eq!(d.para_count(START), 1);
}

#[test]
fn distribute_and_split_alignment_survive_save() {
    for align in [Align::Distribute, Align::Split] {
        let mut d = open(&blank());
        let p = d.insert_text(START, "배분 정렬과 나눔 정렬").unwrap();
        d.set_align(Range::new(START, p).unwrap(), align).unwrap();
        assert_eq!(d.align_at(START), Some(align));
        for fmt in [Format::Hwp, Format::Hwpx] {
            let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
            assert_eq!(back.align_at(START), Some(align), "{align:?} {fmt:?}");
        }
    }
}

#[test]
fn selection_rects_cover_selected_text() {
    let mut d = open(&blank());
    d.insert_text(START, "선택 영역 표시").unwrap();
    let r = Range::new(START.with_offset(1), START.with_offset(5)).unwrap();
    let rects = d.selection_rects(r);
    assert_eq!(rects.len(), 1);
    let a = d.caret_rect(START.with_offset(1)).unwrap();
    let b = d.caret_rect(START.with_offset(5)).unwrap();
    assert!((rects[0].x - a.x).abs() < 0.01 && (rects[0].w - (b.x - a.x)).abs() < 0.01);
    assert!(d
        .selection_rects(Range::new(START, START).unwrap())
        .is_empty());
}

#[test]
fn italic_is_stored_and_rendered_as_italic_text() {
    let mut d = open(&blank());
    d.insert_text(START, "기울임abc").unwrap();
    let r = Range::new(START, START.with_offset(6)).unwrap();
    d.apply_char_format(
        r,
        &CharFormat {
            italic: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(d.char_state(START.with_offset(1)).unwrap().italic);
    let svg = d.render_page_svg(0).unwrap();
    let i = svg.find("italic").expect("svg marks italic glyphs");
    eprintln!("{}", &svg[svg[..i].rfind('<').unwrap()..i + 80]);
    for fmt in [Format::Hwp, Format::Hwpx] {
        let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
        assert!(
            back.char_state(START.with_offset(1)).unwrap().italic,
            "{fmt:?}"
        );
    }
}

use hwpedit_engine::{LineSpacingKind, ParaFormat};

#[test]
fn copy_cut_paste_keeps_formatting_and_is_one_undo_step() {
    let mut d = open(&blank());
    d.insert_text(START, "굵은글자 보통").unwrap();
    let bold = Range::new(START, START.with_offset(4)).unwrap();
    d.apply_char_format(
        bold,
        &CharFormat {
            bold: Some(true),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(d.copy(bold).unwrap(), "굵은글자");
    assert_eq!(d.internal_clipboard_text().as_deref(), Some("굵은글자"));
    let end = START.with_offset(d.para_len(START));
    let after = d.paste_internal(end, None).unwrap();
    assert_eq!(d.body_text()[0], "굵은글자 보통굵은글자");
    assert_eq!(after, START.with_offset(11));
    assert!(
        d.char_state(START.with_offset(9)).unwrap().bold,
        "pasted text keeps bold"
    );

    d.undo(after).unwrap();
    assert_eq!(d.body_text()[0], "굵은글자 보통");

    // paste over a selection replaces it
    let sel = Range::new(START.with_offset(5), START.with_offset(7)).unwrap();
    d.paste_internal(sel.end, Some(sel)).unwrap();
    assert_eq!(d.body_text()[0], "굵은글자 굵은글자");
}

#[test]
fn paste_plain_text_splits_lines_into_paragraphs() {
    let mut d = open(&blank());
    let p = d.paste_text(START, None, "첫 줄\r\n둘째 줄\n셋째").unwrap();
    assert_eq!(&d.body_text()[..3], ["첫 줄", "둘째 줄", "셋째"]);
    assert_eq!(
        p,
        Pos::Body {
            section: 0,
            para: 2,
            offset: 2
        }
    );
    d.undo(p).unwrap();
    assert_eq!(d.body_text()[0], "");
    assert!(!d.edited());
}

#[test]
fn formatting_chosen_before_typing_applies_to_inserted_text_only() {
    let mut d = open(&blank());
    let p = d.insert_text(START, "보통").unwrap();
    let f = CharFormat {
        bold: Some(true),
        size_pt: Some(20.0),
        ..Default::default()
    };
    let q = d.insert_text_formatted(p, "굵", &f).unwrap();
    let q = d.insert_text(q, "게").unwrap(); // continues the formatting it follows
    assert_eq!(d.body_text()[0], "보통굵게");
    assert!(!d.char_state(START.with_offset(2)).unwrap().bold);
    let s = d.char_state(START.with_offset(3)).unwrap();
    assert!(s.bold && s.size_pt == 20.0);
    assert!(d.char_state(q).unwrap().bold);
    for fmt in [Format::Hwp, Format::Hwpx] {
        let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
        assert!(
            back.char_state(START.with_offset(4)).unwrap().bold,
            "{fmt:?}"
        );
        assert!(
            !back.char_state(START.with_offset(2)).unwrap().bold,
            "{fmt:?}"
        );
    }
}

#[test]
fn paragraph_format_line_spacing_margins_indent() {
    let mut d = open(&blank());
    let p = d.insert_text(START, "문단 서식").unwrap();
    let r = Range { start: p, end: p };
    let before = d.caret_rect(START).unwrap();
    let f = ParaFormat {
        line_spacing_pct: Some(200),
        margin_left_pt: Some(36.0),
        margin_right_pt: Some(10.0),
        indent_pt: Some(12.0),
        ..Default::default()
    };
    d.set_para_format(r, &f).unwrap();
    let st = d.para_state(START).unwrap();
    assert_eq!(st.line_spacing_pct, Some(200.0));
    assert!((st.margin_left_pt - 36.0).abs() < 0.01, "{st:?}");
    assert!((st.margin_right_pt - 10.0).abs() < 0.01, "{st:?}");
    assert!((st.indent_pt - 12.0).abs() < 0.01, "{st:?}");
    // first line: 36pt margin + 12pt indent = 48pt = 64px
    let after = d.caret_rect(START).unwrap();
    assert!(
        (after.x - before.x - 64.0).abs() < 0.6,
        "{before:?} -> {after:?}"
    );
    // hanging indent (내어쓰기) is stored as negative; the first line stays at the margin
    d.set_para_format(
        r,
        &ParaFormat {
            indent_pt: Some(-12.0),
            ..Default::default()
        },
    )
    .unwrap();
    assert!((d.para_state(START).unwrap().indent_pt + 12.0).abs() < 0.01);
    assert!((d.caret_rect(START).unwrap().x - before.x - 48.0).abs() < 0.6);
    for fmt in [Format::Hwp, Format::Hwpx] {
        let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
        let s = back.para_state(START).unwrap();
        assert_eq!(s.line_spacing_pct, Some(200.0), "{fmt:?}");
        assert!(
            (s.margin_left_pt - 36.0).abs() < 0.01 && (s.indent_pt + 12.0).abs() < 0.01,
            "{fmt:?} {s:?}"
        );
        assert!((s.margin_right_pt - 10.0).abs() < 0.01, "{fmt:?} {s:?}");
    }
}

#[test]
fn paragraph_spacing_line_kinds_and_breaks() {
    let mut d = open(&blank());
    let a = d.insert_text(START, "첫째 문단").unwrap();
    let b = d.split_paragraph(a).unwrap();
    let end = d.insert_text(b, "둘째 문단").unwrap();
    let second = |d: &Document| d.caret_rect(b).unwrap().y;
    let y0 = second(&d);
    // 문단 아래 간격 10pt on the first paragraph pushes the second down 10pt = 13.3px
    let one = Range {
        start: START,
        end: START,
    };
    d.set_para_format(
        one,
        &ParaFormat {
            spacing_after_pt: Some(10.0),
            ..Default::default()
        },
    )
    .unwrap();
    let st = d.para_state(START).unwrap();
    assert!((st.spacing_after_pt - 10.0).abs() < 0.01, "{st:?}");
    assert!(
        (second(&d) - y0 - 13.33).abs() < 0.7,
        "{y0} -> {}",
        second(&d)
    );
    // 문단 위 간격 6pt on the second: another 8px
    let two = Range { start: b, end };
    d.set_para_format(
        two,
        &ParaFormat {
            spacing_before_pt: Some(6.0),
            line_spacing: Some((LineSpacingKind::Fixed, 30.0)),
            keep_with_next: Some(true),
            page_break_before: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let st = d.para_state(b).unwrap();
    assert!((st.spacing_before_pt - 6.0).abs() < 0.01, "{st:?}");
    assert_eq!(st.line_spacing.0, LineSpacingKind::Fixed);
    assert!((st.line_spacing.1 - 30.0).abs() < 0.01, "{st:?}");
    assert_eq!(st.line_spacing_pct, None);
    assert!(
        st.keep_with_next && st.page_break_before && !st.keep_lines,
        "{st:?}"
    );
    // 문단 앞에서 항상 쪽 나눔
    assert_eq!(d.page_count(), 2);
    for fmt in [Format::Hwp, Format::Hwpx] {
        let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
        let s = back.para_state(b).unwrap();
        assert_eq!(s.line_spacing.0, LineSpacingKind::Fixed, "{fmt:?}");
        assert!((s.line_spacing.1 - 30.0).abs() < 0.01, "{fmt:?} {s:?}");
        assert!((s.spacing_before_pt - 6.0).abs() < 0.01, "{fmt:?} {s:?}");
        assert!(s.keep_with_next && s.page_break_before, "{fmt:?} {s:?}");
        let f = back.para_state(START).unwrap();
        assert!((f.spacing_after_pt - 10.0).abs() < 0.01, "{fmt:?} {f:?}");
        assert_eq!(back.page_count(), 2, "{fmt:?}");
    }
    d.undo(b).unwrap();
    assert_eq!(
        d.para_state(b).unwrap().line_spacing.0,
        LineSpacingKind::Percent
    );
}

#[test]
fn find_and_replace_all_including_one_undo() {
    let mut d = open(&blank());
    let p = d.insert_text(START, "사과 배 사과").unwrap();
    let q = d.split_paragraph(p).unwrap();
    d.insert_text(q, "사과나무").unwrap();
    let hits = d.find_all("사과", false);
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0], Range::new(START, START.with_offset(2)).unwrap());
    assert_eq!(hits[2].start, q);
    assert_eq!(d.replace_all("사과", "감", false).unwrap(), 3);
    assert_eq!(&d.body_text()[..2], ["감 배 감", "감나무"]);
    assert!(d.find_all("사과", false).is_empty());
    d.undo(START).unwrap();
    assert_eq!(&d.body_text()[..2], ["사과 배 사과", "사과나무"]);
    assert_eq!(d.find_all("APPLE", false).len(), 0);
    d.insert_text(START, "Apple apple ").unwrap();
    assert_eq!(d.find_all("apple", false).len(), 2);
    assert_eq!(d.find_all("apple", true).len(), 1);
}

#[test]
fn text_in_single_paragraph_only() {
    let mut d = open(&blank());
    let p = d.insert_text(START, "찾을 단어").unwrap();
    assert_eq!(
        d.text_in(Range::new(START.with_offset(3), p).unwrap())
            .as_deref(),
        Some("단어")
    );
    let q = d.split_paragraph(p).unwrap();
    assert_eq!(d.text_in(Range::new(START, q).unwrap()), None);
}

#[test]
fn new_blank_document_saves_in_both_formats() {
    for fmt in [Format::Hwp, Format::Hwpx] {
        let mut d = Document::new_blank().unwrap();
        assert!(!d.edited());
        d.set_format(fmt);
        d.insert_text(START, "새 문서").unwrap();
        let bytes = d.serialize(fmt).unwrap().0;
        assert_eq!(detect(&bytes), Ok(fmt));
        assert_eq!(
            Document::from_bytes(&bytes, fmt).unwrap().body_text()[0],
            "새 문서"
        );
    }
}

#[test]
fn page_break_adds_a_page_and_undoes() {
    let mut d = open(&blank());
    let p = d.insert_text(START, "앞쪽").unwrap();
    let q = d.insert_page_break(p).unwrap();
    assert_eq!(d.page_count(), 2);
    assert_eq!(d.caret_rect(q).unwrap().page, 1);
    d.insert_text(q, "뒤쪽").unwrap();
    for fmt in [Format::Hwp, Format::Hwpx] {
        assert_eq!(
            Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt)
                .unwrap()
                .page_count(),
            2,
            "{fmt:?}"
        );
    }
    d.undo(q).unwrap();
    d.undo(q).unwrap();
    assert_eq!(d.page_count(), 1);
}

#[test]
fn table_create_edit_rows_cols_merge_split_and_save() {
    let mut d = open(&blank());
    let c = d.create_table(START, 2, 3).unwrap();
    let i = d.cell_info(c).unwrap();
    assert_eq!((i.rows, i.cols, i.row, i.col), (2, 3, 0, 0));
    assert!(
        d.caret_rect(c).unwrap().cell.is_some(),
        "cell caret reports cell bounds"
    );
    let c = d.insert_text(c, "가").unwrap();

    let c = d.insert_row(c, true).unwrap();
    assert_eq!(
        d.cell_info(c).map(|i| (i.rows, i.row, i.col)),
        Some((3, 0, 0))
    );
    let c = d.insert_col(c, false).unwrap();
    assert_eq!(
        d.cell_info(c).map(|i| (i.cols, i.row, i.col)),
        Some((4, 0, 1)),
        "caret follows its cell to the right"
    );
    assert_eq!(d.all_text().matches('가').count(), 1);

    // merge the 2x2 block from the caret's cell down-right, then split it again
    let other = d.cell_at(c, 1, 2).unwrap();
    let m = d.merge_cells(c, other).unwrap();
    let mi = d.cell_info(m).unwrap();
    assert_eq!((mi.row, mi.col, mi.row_span, mi.col_span), (0, 1, 2, 2));
    for fmt in [Format::Hwp, Format::Hwpx] {
        let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
        let bi = back.cell_info(m).unwrap();
        assert_eq!(
            (bi.rows, bi.cols, bi.row_span, bi.col_span),
            (3, 4, 2, 2),
            "{fmt:?}"
        );
    }
    let s = d.split_cell(m).unwrap();
    let si = d.cell_info(s).unwrap();
    assert_eq!((si.row_span, si.col_span), (1, 1));
    assert!(d.split_cell(s).is_err(), "nothing left to split");

    let c = d.delete_row(s).unwrap();
    assert_eq!(d.cell_info(c).unwrap().rows, 2);
    let c = d.delete_col(c).unwrap();
    assert_eq!(d.cell_info(c).unwrap().cols, 3);

    // the whole sequence undoes back to a table-less document
    let body = d.delete_table(c).unwrap();
    assert!(d.cell_info(body).is_none());
    while d.can_undo() {
        d.undo(body).unwrap();
    }
    assert!(!d.edited());
    assert!(!d.all_text().contains('가'));
}

#[test]
fn export_pdf_has_one_page_per_document_page() {
    let mut d = open(&blank());
    let p = d.insert_text(START, "PDF 내보내기").unwrap();
    d.insert_page_break(p).unwrap();
    let pdf = d.export_pdf().unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    let pages = String::from_utf8_lossy(&pdf)
        .matches("/Type /Page\n")
        .count()
        + String::from_utf8_lossy(&pdf)
            .matches("/Type /Page ")
            .count()
        + String::from_utf8_lossy(&pdf).matches("/Type/Page/").count();
    let dir = std::env::temp_dir().join(format!("hwpedit-pdf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("out.pdf");
    std::fs::write(&f, &pdf).unwrap();
    let info = std::process::Command::new("pdfinfo").arg(&f).output();
    if let Ok(o) = info {
        let s = String::from_utf8_lossy(&o.stdout);
        assert!(
            s.lines()
                .any(|l| l.starts_with("Pages:") && l.trim_end().ends_with(" 2")),
            "{s}"
        );
    } else {
        assert!(pages >= 2, "pdfinfo unavailable and page objects not found");
    }
    std::fs::remove_dir_all(dir).ok();
}

use hwpedit_engine::Resize;

#[test]
fn tab_moves_through_cells_and_adds_a_row_after_the_last() {
    let mut d = open(&blank());
    let c = d.create_table(START, 2, 2).unwrap();
    let mut p = c;
    let mut seen = vec![];
    for _ in 0..3 {
        let i = d.cell_info(p).unwrap();
        seen.push((i.row, i.col));
        p = d.tab_forward(p).unwrap();
    }
    let i = d.cell_info(p).unwrap();
    seen.push((i.row, i.col));
    assert_eq!(seen, [(0, 0), (0, 1), (1, 0), (1, 1)]);
    assert_eq!(i.rows, 2);
    // Tab in the last cell: one more row, caret in its first cell
    let n = d.tab_forward(p).unwrap();
    let ni = d.cell_info(n).unwrap();
    assert_eq!((ni.rows, ni.row, ni.col), (3, 2, 0));
    // Shift+Tab goes back; nothing before the first cell
    assert_eq!(
        d.cell_info(d.next_cell(n, false).unwrap())
            .map(|i| (i.row, i.col)),
        Some((1, 1))
    );
    assert_eq!(d.next_cell(c, false), None);
    // the added row is one undo step
    d.undo(n).unwrap();
    assert_eq!(d.cell_info(c).unwrap().rows, 2);
}

#[test]
fn resizing_columns_changes_widths_and_survives_save() {
    let mut d = open(&blank());
    let c = d.create_table(START, 2, 3).unwrap();
    let page = d.caret_rect(c).unwrap().page;
    let width = |d: &Document, col: u16| {
        let b = d.cell_boxes(c, page);
        b.iter()
            .find(|b| b.row == 0 && b.col == col)
            .map(|b| b.w)
            .unwrap()
    };
    let total = |d: &Document| {
        d.cell_boxes(c, page)
            .iter()
            .filter(|b| b.row == 0)
            .map(|b| b.w)
            .sum::<f64>()
    };
    let (w0, w1, t0) = (width(&d, 0), width(&d, 1), total(&d));
    assert_eq!(d.cell_boxes(c, page).len(), 6);

    // border mode: column 0 grows, column 1 shrinks, table width unchanged
    let c = d.resize_table(c, false, 4, Resize::Border).unwrap();
    let step_px = 4.0 * hwpedit_engine::RESIZE_STEP_HWPUNIT / hwpedit_engine::HWPUNIT_PER_PX;
    assert!(
        (width(&d, 0) - (w0 + step_px)).abs() < 1.0,
        "{} vs {}",
        width(&d, 0),
        w0 + step_px
    );
    assert!((width(&d, 1) - (w1 - step_px)).abs() < 1.0);
    assert!((total(&d) - t0).abs() < 1.0);

    // table mode: column 0 grows and the table with it
    let c = d.resize_table(c, false, 2, Resize::Table).unwrap();
    assert!(total(&d) > t0 + 5.0);
    let after = (width(&d, 0), width(&d, 1));
    for fmt in [Format::Hwp, Format::Hwpx] {
        let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
        let bw = |col: u16| {
            back.cell_boxes(c, page)
                .iter()
                .find(|b| b.row == 0 && b.col == col)
                .map(|b| b.w)
                .unwrap()
        };
        assert!(
            (bw(0) - after.0).abs() < 1.0 && (bw(1) - after.1).abs() < 1.0,
            "{fmt:?}"
        );
    }

    // equal widths keep the total
    let t = total(&d);
    let c = d.equalize_columns(c).unwrap();
    let ws: Vec<f64> = (0..3).map(|i| width(&d, i)).collect();
    assert!(ws.iter().all(|w| (w - ws[0]).abs() < 1.0), "{ws:?}");
    assert!((total(&d) - t).abs() < 1.0);

    // each resize is one undo step: three undos restore the original widths
    for _ in 0..3 {
        d.undo(c).unwrap();
    }
    assert!((width(&d, 0) - w0).abs() < 0.5 && (width(&d, 1) - w1).abs() < 0.5);
}

#[test]
fn row_height_grows() {
    let mut d = open(&blank());
    let c = d.create_table(START, 2, 2).unwrap();
    let page = d.caret_rect(c).unwrap().page;
    let h = |d: &Document| {
        d.cell_boxes(c, page)
            .iter()
            .find(|b| b.row == 0 && b.col == 0)
            .map(|b| b.h)
            .unwrap()
    };
    let h0 = h(&d);
    d.resize_table(c, true, 5, Resize::Table).unwrap();
    assert!(h(&d) > h0 + 3.0, "{} -> {}", h0, h(&d));
}

#[test]
fn scaling_the_whole_table_from_its_corner() {
    let mut d = open(&blank());
    let c = d.create_table(START, 3, 3).unwrap();
    let page = d.caret_rect(c).unwrap().page;
    let size = |d: &Document| {
        let b = d.cell_boxes(c, page);
        let x0 = b.iter().map(|b| b.x).fold(f64::MAX, f64::min);
        let y0 = b.iter().map(|b| b.y).fold(f64::MAX, f64::min);
        let x1 = b.iter().map(|b| b.x + b.w).fold(0.0, f64::max);
        let y1 = b.iter().map(|b| b.y + b.h).fold(0.0, f64::max);
        (x1 - x0, y1 - y0)
    };
    let (w0, h0) = size(&d);
    let (c, rows_ok) = d.scale_table(c, 0.5, 2.0).unwrap();
    assert!(rows_ok);
    let (w1, h1) = size(&d);
    assert!((w1 - w0 * 0.5).abs() < 2.0, "{w0} -> {w1}");
    assert!(h1 > h0 * 1.6, "{h0} -> {h1}");
    // columns stay proportional (all equal here)
    let ws: Vec<f64> = d
        .cell_boxes(c, page)
        .iter()
        .filter(|b| b.row == 0)
        .map(|b| b.w)
        .collect();
    assert!(ws.iter().all(|w| (w - ws[0]).abs() < 1.0), "{ws:?}");
    for fmt in [Format::Hwp, Format::Hwpx] {
        let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
        let bw: f64 = back
            .cell_boxes(c, page)
            .iter()
            .filter(|b| b.row == 0)
            .map(|b| b.w)
            .sum();
        assert!((bw - w1).abs() < 2.0, "{fmt:?}");
    }
    // one undo step restores the original size
    d.undo(c).unwrap();
    let (w2, h2) = size(&d);
    assert!((w2 - w0).abs() < 0.5 && (h2 - h0).abs() < 0.5);
}

#[test]
fn table_copy_paste_and_equal_row_heights() {
    let mut d = open(&blank());
    let c = d.create_table(START, 2, 2).unwrap();
    let c = d.insert_text(c, "원본").unwrap();
    d.copy_table(c).unwrap();
    assert!(d.clipboard_has_object());
    // paste in the paragraph after the table
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
    let n_before = d.all_text().matches("원본").count();
    d.paste_object(after).unwrap();
    assert_eq!(
        d.all_text().matches("원본").count(),
        n_before + 1,
        "{}",
        d.all_text()
    );
    for fmt in [Format::Hwp, Format::Hwpx] {
        let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
        assert_eq!(back.all_text().matches("원본").count(), 2, "{fmt:?}");
    }

    // make row 0 tall, then equalize
    let page = d.caret_rect(c).unwrap().page;
    let first = d.cell_at(c, 0, 0).unwrap();
    d.resize_table(first, true, 8, Resize::Table).unwrap();
    let hs = |d: &Document| {
        d.cell_boxes(first, page)
            .iter()
            .filter(|b| b.col == 0)
            .map(|b| b.h)
            .collect::<Vec<_>>()
    };
    let before = hs(&d);
    assert!(before[0] > before[1] + 10.0, "{before:?}");
    d.equalize_rows(first).unwrap();
    let after = hs(&d);
    assert!((after[0] - after[1]).abs() < 4.0, "{before:?} -> {after:?}");
}

#[test]
fn deleting_a_table_with_a_picture_is_not_a_save_loss() {
    use hwpedit_engine::loss::{detect_losses, detect_losses_allowing};
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/rhwp/samples/tb-img-03.hwp"
    );
    let orig = std::fs::read(p).expect("vendor/rhwp/samples/tb-img-03.hwp");
    let mut d = open(&orig);
    let table = Pos::Cell {
        section: 0,
        parent_para: 0,
        control: 2,
        cell: 0,
        cell_para: 0,
        offset: 0,
    };
    assert!(d.cell_info(table).is_some(), "sample layout changed");
    d.delete_table(table).unwrap();
    let saved = d.serialize(Format::Hwp).unwrap().0;
    let plain = detect_losses(&orig, &saved, Format::Hwp).unwrap();
    assert!(
        plain.iter().any(|l| l.what.contains("그림")),
        "the table held a picture: {plain:?}"
    );
    assert_eq!(
        detect_losses_allowing(&orig, &saved, Format::Hwp, d.removed_objects()).unwrap(),
        []
    );

    // the allowance follows undo/redo and is reset by a save
    let removed = d.removed_objects().clone();
    d.undo(START).unwrap();
    assert!(d.removed_objects().is_empty());
    d.redo(START).unwrap();
    assert_eq!(d.removed_objects(), &removed);
    d.mark_saved();
    assert!(d.removed_objects().is_empty());
}

/// Top and bottom (px) of the table at `t` on page 0, from the rendered cell boxes.
fn table_span(d: &Document, t: Pos) -> (f64, f64) {
    let b = d.cell_boxes(t, 0);
    assert!(!b.is_empty(), "table not on page 1");
    (
        b.iter().map(|c| c.y).fold(f64::MAX, f64::min),
        b.iter().map(|c| c.y + c.h).fold(f64::MIN, f64::max),
    )
}

fn type_in(d: &mut Document, mut p: Pos, s: &str) -> Pos {
    for ch in s.chars() {
        p = d.insert_text(p, &ch.to_string()).unwrap();
    }
    p
}

#[test]
fn a_table_created_below_another_does_not_overlap_it() {
    for rows in [2u16, 6] {
        let mut d = open(&blank());
        let p = type_in(&mut d, START, "제목");
        let p = d.split_paragraph(p).unwrap();
        let t1 = d.create_table(p, rows, 3).unwrap();
        type_in(&mut d, t1, "첫 표");
        let Pos::Cell { parent_para, .. } = t1 else {
            unreachable!()
        };
        let q = type_in(
            &mut d,
            Pos::Body {
                section: 0,
                para: parent_para + 1,
                offset: 0,
            },
            "표 아래 글",
        );
        let t2 = d.create_table(q, 2, 2).unwrap();
        type_in(&mut d, t2, "둘째 표");
        let (a, b) = (table_span(&d, t1), table_span(&d, t2));
        assert!(
            b.0 >= a.1 - 0.5,
            "{rows} rows: second table {b:?} overlaps first {a:?}"
        );
        // and the same after saving and reopening
        for fmt in [Format::Hwp, Format::Hwpx] {
            let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
            let (a, b) = (table_span(&back, t1), table_span(&back, t2));
            assert!(
                b.0 >= a.1 - 0.5,
                "{rows} rows, {fmt:?} reopened: second table {b:?} overlaps first {a:?}"
            );
        }
    }
}

#[test]
fn a_table_that_grows_pushes_the_next_table_down() {
    let mut d = open(&blank());
    let t1 = d.create_table(START, 2, 2).unwrap();
    let Pos::Cell { parent_para, .. } = t1 else {
        unreachable!()
    };
    let below = Pos::Body {
        section: 0,
        para: parent_para + 1,
        offset: 0,
    };
    let t2 = d.create_table(below, 2, 2).unwrap();
    type_in(&mut d, t2, "둘째 표");
    let check = |d: &Document, what: &str| {
        let (a, b) = (table_span(d, t1), table_span(d, t2));
        assert!(
            b.0 >= a.1 - 0.5,
            "after {what}: second table {b:?} overlaps first {a:?}"
        );
    };
    check(&d, "creating both");
    let c = d.cell_at(t1, 1, 0).unwrap();
    d.insert_row(c, true).unwrap();
    check(&d, "adding a row");
    let c = d.cell_at(t1, 0, 1).unwrap();
    let c = type_in(&mut d, c, "첫 줄");
    let c = d.split_paragraph(c).unwrap();
    type_in(&mut d, c, "둘째 줄");
    check(&d, "Enter in a cell");
    let back = Document::from_bytes(&d.serialize(Format::Hwp).unwrap().0, Format::Hwp).unwrap();
    check(&back, "saving and reopening");
}

#[test]
fn a_long_new_table_splits_across_pages_like_hancom() {
    let mut d = open(&blank());
    let mut p = START;
    for i in 0..15 {
        p = type_in(&mut d, p, &format!("{i}번째 줄"));
        p = d.split_paragraph(p).unwrap();
    }
    let t = d.create_table(p, 40, 2).unwrap();
    assert!(d.page_count() >= 2);
    let on1 = d.cell_boxes(t, 0).len();
    let on2 = d.cell_boxes(t, 1).len();
    assert!(
        on1 > 0 && on2 > 0,
        "table should start on page 1 and continue on page 2: {on1} cells on p1, {on2} on p2"
    );
    // saved like Hancom's default (split at page boundary) and still split after reopening
    let back = Document::from_bytes(&d.serialize(Format::Hwp).unwrap().0, Format::Hwp).unwrap();
    assert!(
        !back.cell_boxes(t, 0).is_empty() && !back.cell_boxes(t, 1).is_empty(),
        "reopened"
    );
}

fn reopen_both(d: &Document) -> Vec<(Format, Document)> {
    [Format::Hwp, Format::Hwpx]
        .into_iter()
        .map(|f| {
            (
                f,
                Document::from_bytes(&d.serialize(f).unwrap().0, f).unwrap(),
            )
        })
        .collect()
}

#[test]
fn page_setup_landscape_and_margins_survive_save() {
    let mut d = open(&blank());
    d.insert_text(START, "가로 용지").unwrap();
    let mut p = d.page_setup(0).unwrap();
    assert!(!p.landscape);
    let portrait_w = d.page_size(0).unwrap().width;
    p.landscape = true;
    p.margin_left_mm = 20.0;
    d.set_page_setup(START, &p).unwrap();
    let pg = d.page_size(0).unwrap();
    assert!(
        pg.width > pg.height,
        "landscape page should be wider than tall: {}x{}",
        pg.width,
        pg.height
    );
    assert!(pg.width > portrait_w);
    for (f, back) in reopen_both(&d) {
        let q = back.page_setup(0).unwrap();
        assert!(q.landscape, "{f:?}");
        assert!(
            (q.margin_left_mm - 20.0).abs() < 0.05,
            "{f:?}: {}",
            q.margin_left_mm
        );
    }
    d.undo(START).unwrap();
    assert!(!d.page_setup(0).unwrap().landscape);
}

#[test]
fn columns_and_column_break_survive_save() {
    let mut d = open(&blank());
    let p = d.insert_text(START, "첫째 단").unwrap();
    d.set_columns(START, 2, 8.0).unwrap();
    let q = d.insert_column_break(p).unwrap();
    d.insert_text(q, "둘째 단").unwrap();
    for (f, back) in reopen_both(&d) {
        let t = back.body_text().join("|");
        assert!(t.contains("첫째 단") && t.contains("둘째 단"), "{f:?}: {t}");
        assert_eq!(back.page_count(), 1, "{f:?}");
    }
    // the second column starts to the right of the first
    let a = d.caret_rect(START).unwrap();
    let b = d.caret_rect(q).unwrap();
    assert!(
        b.x > a.x + 100.0 && (b.y - a.y).abs() < 2.0,
        "first {a:?} second {b:?}"
    );
}

#[test]
fn picture_bookmark_and_link_survive_save() {
    // 2x1 red/blue PNG
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x7b,
        0x40, 0xe8, 0xdd, 0x00, 0x00, 0x00, 0x0f, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0xc0, 0xf0, 0x1f, 0x08, 0x00, 0x1d, 0xf0, 0x03, 0xfd, 0xb9, 0x6b, 0x2b, 0x6e,
        0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let mut d = open(&blank());
    let p = d.insert_text(START, "링크 글자").unwrap();
    d.insert_hyperlink(
        Range::new(START, START.with_offset(2)).unwrap(),
        "https://example.com/",
    )
    .unwrap();
    d.add_bookmark(p, "끝").unwrap();
    assert_eq!(d.bookmarks(), vec!["끝".to_owned()]);
    let at = d.insert_picture(p, PNG, "png", (2, 1), 40.0).unwrap();
    assert_eq!(at, p);
    for (f, back) in reopen_both(&d) {
        assert!(back.body_text()[0].contains("링크 글자"), "{f:?}");
        assert_eq!(back.bookmarks(), vec!["끝".to_owned()], "{f:?}");
        assert_eq!(back.picture_count(), 1, "{f:?}");
    }
}

#[test]
fn notes_and_header_footer_survive_save() {
    let mut d = open(&blank());
    let p = d.insert_text(START, "본문").unwrap();
    let q = d.insert_note(p, false, "각주 내용").unwrap();
    assert_eq!(q, p);
    d.insert_note(q, true, "미주 내용").unwrap();
    d.set_header_footer_text(START, true, "머리말 글").unwrap();
    d.set_header_footer_text(START, false, "꼬리말 글").unwrap();
    // setting again replaces the text
    d.set_header_footer_text(START, true, "새 머리말").unwrap();
    assert_eq!(
        d.header_footer_text(START, true).as_deref(),
        Some("새 머리말")
    );
    for (f, back) in reopen_both(&d) {
        assert_eq!(
            back.header_footer_text(START, true).as_deref(),
            Some("새 머리말"),
            "{f:?}"
        );
        assert_eq!(
            back.header_footer_text(START, false).as_deref(),
            Some("꼬리말 글"),
            "{f:?}"
        );
        assert!(back.body_text()[0].starts_with("본문"), "{f:?}");
    }
    d.undo(START).unwrap();
    assert_eq!(
        d.header_footer_text(START, true).as_deref(),
        Some("머리말 글")
    );
}

use hwpedit_engine::{ListKind, ListState};

/// Three paragraphs 하나 / 둘 / 셋; returns the start of each.
fn three_paras(d: &mut Document) -> [Pos; 3] {
    let a = d.insert_text(START, "하나").unwrap();
    let b = d.split_paragraph(a).unwrap();
    let b2 = d.insert_text(b, "둘").unwrap();
    let c = d.split_paragraph(b2).unwrap();
    d.insert_text(c, "셋").unwrap();
    [START, b, c]
}

#[test]
fn numbering_bullets_and_levels_survive_save() {
    let mut d = open(&blank());
    let [a, b, c] = three_paras(&mut d);
    d.set_list(Range::new(a, b).unwrap(), ListKind::Number, '●')
        .unwrap();
    d.set_list(Range::new(c, c).unwrap(), ListKind::Bullet, '■')
        .unwrap();
    assert!(d.change_level(Range::new(b, b).unwrap(), 1).unwrap());
    let expect = [
        ListState {
            kind: ListKind::Number,
            level: 0,
        },
        ListState {
            kind: ListKind::Number,
            level: 1,
        },
        ListState {
            kind: ListKind::Bullet,
            level: 0,
        },
    ];
    for (p, want) in [a, b, c].into_iter().zip(expect) {
        assert_eq!(d.list_at(p), Some(want), "{p:?}");
    }
    for (f, back) in reopen_both(&d) {
        for (p, want) in [a, b, c].into_iter().zip(expect) {
            assert_eq!(back.list_at(p), Some(want), "{f:?} {p:?}");
        }
        assert_eq!(back.body_text(), d.body_text(), "{f:?}");
    }
    // the heads are drawn like Hwp's: 1. / 가. / ■
    let svg = d.render_page_svg(0).unwrap();
    let glyphs: String = svg
        .split("</text>")
        .filter_map(|t| t.rsplit('>').next())
        .collect();
    assert_eq!(glyphs.trim_end(), "1.하나가.둘■셋");

    // the first line carries Hwp's 문단 머리 flag (LINE_SEG bit 21), also after typing
    d.insert_text(c.with_offset(1), "넷").unwrap();
    for (f, back) in reopen_both(&d) {
        for (p, head) in [(a, true), (b, true), (c, true)] {
            assert_eq!(back.first_line_has_head_tag(p), Some(head), "{f:?} {p:?}");
        }
    }

    // 해제, then undo brings the head back
    d.set_list(Range::new(a, a).unwrap(), ListKind::None, '●')
        .unwrap();
    assert_eq!(d.list_at(a).unwrap().kind, ListKind::None);
    assert_eq!(d.first_line_has_head_tag(a), Some(false));
    d.undo(START).unwrap();
    assert_eq!(d.list_at(a).unwrap().kind, ListKind::Number);

    // numbered paragraphs count apart from 개요 (Hwp 2022: 개요 "1." then 문단 번호 "1.")
    let mut e = open(&blank());
    let [a, b, _] = three_paras(&mut e);
    e.apply_outline(Range::new(a, a).unwrap(), 0).unwrap();
    e.set_list(Range::new(b, b).unwrap(), ListKind::Number, '●')
        .unwrap();
    let svg = e.render_page_svg(0).unwrap();
    let glyphs: String = svg
        .split("</text>")
        .filter_map(|t| t.rsplit('>').next())
        .collect();
    assert_eq!(glyphs.trim_end(), "1.하나1.둘셋");
    // a second 문단 번호 reuses the same shape, so the two count on: 1. 2.
    e.set_list(Range::new(a, a).unwrap(), ListKind::Number, '●')
        .unwrap();
    let svg = e.render_page_svg(0).unwrap();
    let glyphs: String = svg
        .split("</text>")
        .filter_map(|t| t.rsplit('>').next())
        .collect();
    assert_eq!(glyphs.trim_end(), "1.하나2.둘셋");
}

#[test]
fn styles_and_outline_levels_survive_save() {
    let mut d = open(&blank());
    let [a, b, _] = three_paras(&mut d);
    let styles = d.styles();
    let id = |name: &str| {
        styles
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no style {name}"))
            .id
    };
    assert_eq!(d.style_at(a), Some(id("바탕글")));
    d.apply_outline(Range::new(a, a).unwrap(), 0).unwrap();
    d.apply_style(Range::new(b, b).unwrap(), id("본문"))
        .unwrap();
    assert_eq!(d.list_at(a).unwrap().kind, ListKind::Outline);
    assert!(d.change_level(Range::new(a, a).unwrap(), 1).unwrap());
    assert_eq!(d.style_at(a), Some(id("개요 2")));
    // as in Hwp 2022 (checked with Ctrl+Num -/+): 개요 1 증가 -> 바탕글, 바탕글 감소 -> 개요 1,
    // a top-level number 증가 stays
    let mut e = open(&blank());
    e.insert_text(START, "x").unwrap();
    let at = Range::new(START, START).unwrap();
    assert!(e.change_level(at, 1).unwrap());
    assert_eq!(e.style_at(START), Some(id("개요 1")));
    assert!(e.change_level(at, -1).unwrap());
    assert_eq!(e.style_at(START), Some(id("바탕글")));
    e.set_list(at, ListKind::Number, '●').unwrap();
    assert!(!e.change_level(at, -1).unwrap());
    for (f, back) in reopen_both(&d) {
        assert_eq!(back.style_at(a), Some(id("개요 2")), "{f:?}");
        assert_eq!(back.style_at(b), Some(id("본문")), "{f:?}");
        assert_eq!(
            back.list_at(a).map(|l| (l.kind, l.level)),
            Some((ListKind::Outline, 1)),
            "{f:?}"
        );
    }
}

use hwpedit_engine::{BorderLine, BorderSides};

#[test]
fn cell_shading_and_borders_survive_save() {
    let mut d = open(&blank());
    let c = d.create_table(START, 3, 3).unwrap();
    let at = |d: &Document, r, k| d.cell_at(c, r, k).unwrap();
    let before = d.cell_look(at(&d, 1, 1)).unwrap();
    assert_eq!(before.fill, None);
    assert_eq!(before.borders[0].kind, 1, "a new table has solid borders");

    // 셀 음영 on the top-left 2x2 block, a red double line around the whole table
    d.set_cell_fill(at(&d, 0, 0), at(&d, 1, 1), Some("#ffff00"))
        .unwrap();
    let red = BorderLine {
        kind: 8,
        width: 3,
        color: "#ff0000".into(),
    };
    d.set_cell_borders(at(&d, 0, 0), at(&d, 2, 2), BorderSides::Outer, &red)
        .unwrap();
    // no line between the two bottom-right cells
    let none = BorderLine {
        kind: 0,
        width: 0,
        color: "#000000".into(),
    };
    d.set_cell_borders(
        at(&d, 2, 1),
        at(&d, 2, 2),
        BorderSides::InnerVertical,
        &none,
    )
    .unwrap();

    let check = |d: &Document, f: &str| {
        let look = |r, k| d.cell_look(d.cell_at(c, r, k).unwrap()).unwrap();
        assert_eq!(look(0, 0).fill.as_deref(), Some("#ffff00"), "{f}");
        assert_eq!(look(1, 1).fill.as_deref(), Some("#ffff00"), "{f}");
        assert_eq!(look(2, 2).fill, None, "{f}");
        let (tl, mid) = (look(0, 0), look(1, 1));
        assert_eq!(
            (&tl.borders[0], &tl.borders[2]),
            (&red, &red),
            "{f}: outer left/top"
        );
        assert_eq!(tl.borders[1].kind, 1, "{f}: inner edge stays solid");
        assert!(
            mid.borders.iter().all(|b| b.kind == 1),
            "{f}: middle cell untouched"
        );
        assert_eq!(look(2, 2).borders[1], red, "{f}: outer right");
        // the shared edge is the same seen from both cells
        assert_eq!(look(2, 1).borders[1].kind, 0, "{f}");
        assert_eq!(look(2, 2).borders[0].kind, 0, "{f}");
    };
    check(&d, "edited");
    for (f, back) in reopen_both(&d) {
        check(&back, &format!("{f:?}"));
    }
    // one undo step per command
    d.undo(c).unwrap();
    assert_eq!(
        d.cell_look(d.cell_at(c, 2, 2).unwrap()).unwrap().borders[0].kind,
        1
    );
}

use hwpedit_engine::PageNumber;

/// The page's text as drawn, glyph by glyph.
fn page_glyphs(d: &Document, page: u32) -> String {
    let svg = d.render_page_svg(page).unwrap();
    svg.split("</text>")
        .filter_map(|t| t.rsplit('>').next())
        .collect::<String>()
        .trim_end()
        .to_owned()
}

#[test]
fn page_numbers_and_new_number_survive_save() {
    let mut d = open(&blank());
    let a = d.insert_text(START, "첫째").unwrap();
    let b = d.insert_page_break(a).unwrap();
    let b2 = d.insert_text(b, "둘째").unwrap();
    let c = d.insert_page_break(b2).unwrap();
    d.insert_text(c, "셋째").unwrap();
    assert_eq!(d.page_count(), 3);
    assert_eq!(d.page_number(0), None);

    let n = PageNumber {
        position: 5,
        format: 0,
        dash: true,
    };
    d.set_page_number(START, &n).unwrap();
    // setting it again changes it instead of adding a second one
    let roman = PageNumber {
        position: 6,
        format: 2,
        dash: false,
    };
    d.set_page_number(c, &roman).unwrap();
    assert_eq!(d.page_number(0), Some(roman));
    d.set_page_number(START, &n).unwrap();
    assert!(
        page_glyphs(&d, 0).ends_with("-1-"),
        "{}",
        page_glyphs(&d, 0)
    );
    assert!(
        page_glyphs(&d, 2).ends_with("-3-"),
        "{}",
        page_glyphs(&d, 2)
    );
    d.set_new_page_number(b, 10).unwrap();
    d.set_new_page_number(b, 20).unwrap();
    assert_eq!(d.new_page_number_at(b), Some(20));

    let check = |d: &Document, f: &str| {
        assert_eq!(d.page_number(0), Some(n), "{f}");
        assert_eq!(d.new_page_number_at(b), Some(20), "{f}");
        assert_eq!(d.page_count(), 3, "{f}");
        // as Hwp 2022 prints it (feature_lab f08): - 1 -, then 20 and 21 from the 새 번호
        assert!(
            page_glyphs(d, 0).ends_with("-1-"),
            "{f}: {}",
            page_glyphs(d, 0)
        );
        assert!(
            page_glyphs(d, 1).ends_with("-20-"),
            "{f}: {}",
            page_glyphs(d, 1)
        );
        assert!(
            page_glyphs(d, 2).ends_with("-21-"),
            "{f}: {}",
            page_glyphs(d, 2)
        );
    };
    check(&d, "edited");
    for (f, back) in reopen_both(&d) {
        check(&back, &format!("{f:?}"));
    }
    assert_eq!(d.body_text(), ["첫째", "둘째", "셋째"]);
}

use hwpedit_engine::PageBorder;

#[test]
fn page_border_and_background_survive_save() {
    let mut d = open(&blank());
    d.insert_text(START, "쪽 테두리").unwrap();
    let before = d.page_border(0).unwrap();
    assert!(
        before.borders.iter().all(|b| b.kind == 0),
        "a blank page has no border: {before:?}"
    );
    let blue = BorderLine {
        kind: 1,
        width: 7,
        color: "#0000ff".into(),
    };
    let set = PageBorder {
        borders: [blue.clone(), blue.clone(), blue.clone(), blue],
        fill: Some("#fff2cc".into()),
        page_based: true,
        spacing_mm: [5.0, 5.0, 5.0, 5.0],
        header_inside: false,
        footer_inside: false,
    };
    d.set_page_border(START, &set).unwrap();
    assert_eq!(d.page_border(0).as_ref(), Some(&set));
    for (f, back) in reopen_both(&d) {
        assert_eq!(back.page_border(0).as_ref(), Some(&set), "{f:?}");
    }
    d.undo(START).unwrap();
    assert!(d
        .page_border(0)
        .unwrap()
        .borders
        .iter()
        .all(|b| b.kind == 0));
}

use hwpedit_engine::latex;

#[test]
fn equations_from_latex_survive_save() {
    let mut d = open(&blank());
    let a = d.insert_text(START, "근의 공식: ").unwrap();
    let quad = latex::to_hwp(r"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}").unwrap();
    assert!(quad.unknown.is_empty());
    assert_eq!(quad.script, "x = {- b +- sqrt {b ^{2} - 4 a c}} over {2 a}");
    let after = d.insert_equation(a, &quad.script).unwrap();
    let b = d.insert_text_after_object(after, " 그리고 ").unwrap();
    let sum = latex::to_hwp(r"\sum_{k=1}^{n} k").unwrap().script;
    let end = d.insert_equation(b, &sum).unwrap();

    // the caret right after an equation finds it; then 수식 고치기
    let (ctrl, script) = d.equation_near(end).unwrap();
    assert_eq!(script, sum);
    let edited = latex::to_hwp(r"\sum_{k=1}^{n} k^2").unwrap().script;
    d.set_equation_script(end, ctrl, &edited).unwrap();
    assert_eq!(
        d.equation_near(after).map(|e| e.1),
        Some(quad.script.clone())
    );

    let svg = d.render_page_svg(0).unwrap();
    assert!(
        svg.contains('√') || svg.contains("sqrt") || svg.matches("<path").count() > 0,
        "the equation is drawn"
    );
    assert!(d
        .equation_preview_svg(&quad.script)
        .unwrap()
        .contains("<svg"));
    for (f, back) in reopen_both(&d) {
        assert_eq!(
            back.equation_near(after).map(|e| e.1),
            Some(quad.script.clone()),
            "{f:?}"
        );
        assert_eq!(
            back.equation_near(end).map(|e| e.1),
            Some(edited.clone()),
            "{f:?}"
        );
        assert_eq!(back.body_text(), d.body_text(), "{f:?}");
    }
    d.undo(end).unwrap();
    assert_eq!(d.equation_near(end).map(|e| e.1), Some(sum));
}

#[test]
fn typing_after_an_inserted_object_goes_after_it() {
    const PNG: &[u8] = include_bytes!("../../app/examples/lab-picture.png");
    // in "ab", an object after "a", then "Q" typed after the object: "a[object]Qb"
    for kind in ["equation", "picture", "footnote"] {
        let mut d = open(&blank());
        d.insert_text(START, "ab").unwrap();
        let at = START.with_offset(1);
        let obj = match kind {
            "equation" => d.insert_equation(at, "x").unwrap(),
            "picture" => d.insert_picture(at, PNG, "png", (120, 60), 20.0).unwrap(),
            _ => d.insert_note(at, false, "n").unwrap(),
        };
        assert_eq!(obj, at, "{kind}");
        let q = d.insert_text_after_object(obj, "Q").unwrap();
        assert_eq!(d.body_text(), ["aQb"], "{kind}");
        assert_eq!(q.offset(), 2, "{kind}");
        if kind == "equation" {
            // the equation stays between a and Q
            assert_eq!(d.equation_near(at).map(|e| e.1), Some("x".into()));
        }
    }
}

#[test]
fn enter_after_an_inline_object_splits_at_the_caret() {
    const PNG: &[u8] = include_bytes!("../../app/examples/lab-picture.png");
    for kind in ["equation", "picture", "footnote"] {
        let put = |d: &mut Document, at: Pos| match kind {
            "equation" => d.insert_equation(at, "x").unwrap(),
            "picture" => d.insert_picture(at, PNG, "png", (120, 60), 20.0).unwrap(),
            _ => d.insert_note(at, false, "n").unwrap(),
        };
        // an object after "a", caret after "abcd": Enter splits "abcd" | "ef"
        let mut d = open(&blank());
        d.insert_text(START, "abcdef").unwrap();
        put(&mut d, START.with_offset(1));
        d.split_paragraph(START.with_offset(4)).unwrap();
        assert_eq!(d.body_text(), ["abcd", "ef"], "{kind}");
        // Enter right after a new object at the end keeps it in the first paragraph
        let mut e = open(&blank());
        e.insert_text(START, "ab").unwrap();
        let obj = put(&mut e, START.with_offset(2));
        let next = e.split_paragraph_after_object(obj).unwrap();
        e.insert_text(next, "cd").unwrap();
        assert_eq!(e.body_text(), ["ab", "cd"], "{kind}");
        if kind == "equation" {
            assert!(
                e.equation_near(START.with_offset(2)).is_some(),
                "the equation stays after ab"
            );
        }
    }
}

#[test]
fn plus_minus_is_drawn_as_one_sign() {
    // Hwp 2022 draws `+-` as ± (feature_lab f09)
    let d = open(&blank());
    let svg = d.equation_preview_svg("a +- b").unwrap();
    assert!(svg.contains('±'), "{svg}");
    for (script, sign) in [
        ("a <= b", '≤'),
        ("a >= b", '≥'),
        ("a != b", '≠'),
        ("x -> 0", '→'),
    ] {
        let svg = d.equation_preview_svg(script).unwrap();
        assert!(svg.contains(sign), "{script}: {svg}");
    }
}

#[test]
fn text_typed_on_the_line_after_an_equation_is_drawn() {
    let mut d = open(&blank());
    let a = d.insert_text(START, "Roots: ").unwrap();
    let eq = d.insert_equation(a, "x = {a} over {b}").unwrap();
    let mut p = d.insert_text_after_object(eq, " ").unwrap();
    for ch in "(done)".chars() {
        p = d.insert_text(p, &ch.to_string()).unwrap();
    }
    let mut q = d.split_paragraph(p).unwrap();
    for ch in "next line".chars() {
        q = d.insert_text(q, &ch.to_string()).unwrap();
    }
    assert_eq!(d.body_text(), ["Roots:  (done)", "next line"]);
    let glyphs = page_glyphs(&d, 0);
    assert!(glyphs.ends_with("nextline"), "{glyphs}");
}

#[test]
fn caret_and_clicks_agree_around_inline_objects() {
    use hwpedit_engine::Hit;
    let mut d = open(&blank());
    d.insert_text(START, "abcdef").unwrap();
    d.insert_equation(START.with_offset(2), "x").unwrap();
    let xs: Vec<f64> = (0..=6)
        .map(|o| d.caret_rect(START.with_offset(o)).unwrap().x)
        .collect();
    // text offsets 2 (before the equation) and 3 (after "c") are an equation plus a letter apart
    let step = xs[1] - xs[0];
    assert!(xs[3] - xs[2] > 1.5 * step, "{xs:?}");
    let after = d.caret_rect_after_object(START.with_offset(2)).unwrap().x;
    assert!(
        after > xs[2] + 0.5 * step && after < xs[3] - 0.5 * step,
        "after the equation: {after} in {xs:?}"
    );
    // a click on each caret position comes back as the same text offset
    for (o, &x) in xs.iter().enumerate() {
        let r = d.caret_rect(START.with_offset(o)).unwrap();
        let (hit, at_object) = d
            .hit_test_at_object(r.page, x + 0.3, r.y + r.height / 2.0)
            .unwrap();
        assert_eq!(hit, Hit::Editable(START.with_offset(o)), "offset {o}");
        assert!(!at_object || o == 2, "offset {o}");
    }
    let r = d.caret_rect(START).unwrap();
    let (hit, at_object) = d
        .hit_test_at_object(r.page, after + 0.3, r.y + r.height / 2.0)
        .unwrap();
    assert_eq!(
        (hit, at_object),
        (Hit::Editable(START.with_offset(2)), true)
    );
}

#[test]
fn backspace_and_delete_remove_an_inline_object() {
    let mut d = open(&blank());
    d.insert_text(START, "ab").unwrap();
    let at = START.with_offset(1);
    d.insert_equation(at, "x").unwrap();
    // Delete with the caret before the equation, Backspace with it after
    assert_eq!(d.object_at(at, false), d.object_at(at, true));
    let c = d.object_at(at, true).unwrap();
    d.delete_object(at, c).unwrap();
    assert_eq!(d.equation_near(at), None);
    assert_eq!(d.body_text(), ["ab"]);
    assert_eq!(d.object_at(at, false), None);
    d.undo(at).unwrap();
    assert!(d.equation_near(at).is_some());
    for (f, back) in reopen_both(&d) {
        assert!(back.equation_near(at).is_some(), "{f:?}");
    }
}

use hwpedit_engine::ShapeKind;

#[test]
fn shapes_and_text_box_survive_save() {
    let mut d = open(&blank());
    let a = d.insert_text(START, "도형: ").unwrap();
    let mut at = a;
    for (kind, w, h) in [
        (ShapeKind::Rectangle, 30.0, 15.0),
        (ShapeKind::Ellipse, 20.0, 20.0),
        (ShapeKind::Line, 40.0, 0.0),
    ] {
        at = d.insert_shape(at, kind, w, h, "").unwrap();
        at = d.insert_text_after_object(at, " ").unwrap();
    }
    d.insert_shape(at, ShapeKind::TextBox, 50.0, 20.0, "글상자 첫 줄\n둘째 줄")
        .unwrap();
    let shapes = d.shapes_at(START);
    assert_eq!(shapes.len(), 4);
    let tb = *shapes.last().unwrap();
    assert_eq!(
        d.text_box_text(START, tb),
        Some(vec!["글상자 첫 줄".to_string(), "둘째 줄".to_string()])
    );
    assert_eq!(d.body_text(), ["도형:    "]);
    for (f, back) in reopen_both(&d) {
        assert_eq!(back.shapes_at(START).len(), 4, "{f:?}");
        assert_eq!(
            back.text_box_text(START, tb),
            Some(vec!["글상자 첫 줄".to_string(), "둘째 줄".to_string()]),
            "{f:?}"
        );
    }
    let glyphs = page_glyphs(&d, 0);
    assert!(
        glyphs.contains("글상자첫줄") && glyphs.contains("둘째줄"),
        "{glyphs}"
    );
    d.undo(at).unwrap();
    assert_eq!(d.shapes_at(START).len(), 3);
}

#[test]
fn equations_in_a_table_cell() {
    let mut d = open(&blank());
    let c = d.create_table(START, 2, 2).unwrap();
    let a = d.insert_text(c, "답: ").unwrap();
    let quad = latex::to_hwp(r"\frac{1}{2}").unwrap().script;
    let eq = d.insert_equation(a, &quad).unwrap();
    assert_eq!(eq, a);
    let after = d.insert_text_after_object(eq, " 끝").unwrap();
    // a second equation in the same cell paragraph, then edit only that one
    let eq2 = d.insert_equation(after, "x").unwrap();
    let (ctrl2, s2) = d.equation_near(eq2).unwrap();
    assert_eq!(s2, "x");
    d.set_equation_script(eq2, ctrl2, "y ^{2}").unwrap();
    assert_eq!(d.equation_near(a).map(|e| e.1), Some(quad.clone()));
    assert_eq!(
        d.equation_near(eq2).map(|e| e.1),
        Some("y ^{2}".to_string())
    );
    // Enter right after the second equation keeps it in this cell paragraph
    let next = d.split_paragraph_after_object(eq2).unwrap();
    d.insert_text(next, "다음 줄").unwrap();
    let cell_text = |d: &Document| d.all_text();
    assert!(
        cell_text(&d).contains("답:  끝\n다음 줄"),
        "{:?}",
        cell_text(&d)
    );
    let glyphs = page_glyphs(&d, 0);
    assert!(glyphs.contains("답:12끝y2다음줄"), "{glyphs}");

    for (f, back) in reopen_both(&d) {
        assert_eq!(
            back.equation_near(a).map(|e| e.1),
            Some(quad.clone()),
            "{f:?}"
        );
        assert_eq!(
            back.equation_near(eq2).map(|e| e.1),
            Some("y ^{2}".to_string()),
            "{f:?}"
        );
        assert_eq!(cell_text(&back), cell_text(&d), "{f:?}");
    }
    // Backspace after the first equation removes it
    let first = d.object_at(a, true).unwrap();
    d.delete_object(a, first).unwrap();
    assert_eq!(d.equation_near(a), None);
    assert!(cell_text(&d).contains("답:  끝\n다음 줄"));
    d.undo(a).unwrap();
    assert_eq!(d.equation_near(a).map(|e| e.1), Some(quad));
}

#[test]
fn double_click_finds_the_equation_under_the_point() {
    let mut d = open(&blank());
    let a = d.insert_text(START, "가").unwrap();
    let e1 = d.insert_equation(a, "x").unwrap();
    let b = d.insert_text_after_object(e1, "나").unwrap();
    let e2 = d.insert_equation(b, "y over 2").unwrap();
    d.insert_text_after_object(e2, "다").unwrap();
    // the second equation's box: between the caret before it and after it
    let r0 = d.caret_rect(e2).unwrap();
    let r1 = d.caret_rect_after_object(e2).unwrap();
    let (x, y) = ((r0.x + r1.x) / 2.0, r0.y + r0.height / 2.0);
    let (at, c) = d
        .equation_at_point(r0.page, x, y)
        .expect("equation under the point");
    assert_eq!(at, e2);
    assert_eq!(d.equation_script(at, c).as_deref(), Some("y over 2"));
    // on the letter 가: no equation
    let g = d.caret_rect(START).unwrap();
    assert_eq!(d.equation_at_point(g.page, g.x + 3.0, y), None);

    // in a table cell
    let mut d = open(&blank());
    let t = d.create_table(START, 1, 2).unwrap();
    let cell = d.cell_at(t, 0, 1).unwrap();
    let a = d.insert_text(cell, "합 ").unwrap();
    let e = d.insert_equation(a, "a+b").unwrap();
    let r0 = d.caret_rect(e).unwrap();
    let r1 = d.caret_rect_after_object(e).unwrap();
    let (at, c) = d
        .equation_at_point(r0.page, (r0.x + r1.x) / 2.0, r0.y + r0.height / 2.0)
        .expect("equation in the cell");
    assert_eq!(at, e);
    assert_eq!(d.equation_script(at, c).as_deref(), Some("a+b"));
}

#[test]
fn caret_follows_the_text_on_lines_after_an_equation() {
    // rhwp lost the equation's place for later runs and lines (patches/0008): in a
    // cell the caret was drawn one letter right of where typing went, and at the end
    // of the paragraph it jumped to the cell's first line
    for in_cell in [false, true] {
        let mut d = open(&blank());
        let c = if in_cell {
            let t = d.create_table(START, 1, 1).unwrap();
            d.cell_at(t, 0, 0).unwrap()
        } else {
            START
        };
        let p = d.insert_text(c, "가나 ").unwrap();
        let e = d.insert_equation(p, "90 DEG").unwrap();
        let p = d
            .insert_text_after_object(e, &"에 다라마바사아자차카타파하 ".repeat(6))
            .unwrap();
        let p = d.insert_text(p, "끝다.").unwrap();
        let len = d.para_len(p);
        let last_line = d.caret_rect(p.with_offset(len - 3)).unwrap();
        let svg = d.render_page_svg(last_line.page).unwrap();
        let glyph_x = |ch: &str| {
            svg.split("<text ")
                .filter(|t| t.contains(&format!(">{ch}</text>")))
                .filter_map(|t| {
                    t.split("x=\"")
                        .nth(1)?
                        .split('"')
                        .next()?
                        .parse::<f64>()
                        .ok()
                })
                .last()
                .unwrap()
        };
        for (o, ch) in [(len - 3, "끝"), (len - 2, "다"), (len - 1, ".")] {
            let r = d.caret_rect(p.with_offset(o)).unwrap();
            assert!(
                (r.x - glyph_x(ch)).abs() < 1.0,
                "cell {in_cell}: caret {o} at {} not before {ch}",
                r.x
            );
            assert_eq!(r.y, last_line.y, "cell {in_cell}: offset {o}");
            let (hit, _) = d
                .hit_test_at_object(r.page, r.x + 0.5, r.y + r.height / 2.0)
                .unwrap();
            assert_eq!(
                hit,
                hwpedit_engine::Hit::Editable(p.with_offset(o)),
                "cell {in_cell}"
            );
        }
        let end = d.caret_rect(p.with_offset(len)).unwrap();
        assert!(
            end.y == last_line.y && end.x > glyph_x("."),
            "cell {in_cell}: end {end:?}"
        );
    }
}

#[test]
fn paragraph_marks_are_screen_only() {
    let mut d = open(&blank());
    let a = d.insert_text(START, "첫째").unwrap();
    let b = d.split_paragraph(a).unwrap();
    d.insert_text(b, "둘째").unwrap();
    let plain = d.render_page_svg(0).unwrap();
    let saved = d.serialize(Format::Hwp).unwrap().0;
    let caret = d.caret_rect(b).unwrap();
    d.set_view_marks(true, false);
    let marked = d.render_page_svg(0).unwrap();
    assert_ne!(plain, marked, "marks drawn");
    assert_eq!(d.serialize(Format::Hwp).unwrap().0, saved, "file unchanged");
    assert_eq!(d.caret_rect(b).unwrap(), caret, "caret unchanged");
    d.set_view_marks(false, false);
    assert_eq!(d.render_page_svg(0).unwrap(), plain);
}

#[test]
fn insert_document_keeps_both_texts_and_formats() {
    // the other document: bold text, a table and a 수식
    let mut other = open(&blank());
    let a = other.insert_text(START, "끼울 글").unwrap();
    other
        .apply_char_format(
            Range::new(START, a).unwrap(),
            &CharFormat {
                bold: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    let b = other.split_paragraph(a).unwrap();
    let t = other.create_table(b, 2, 2).unwrap();
    let c = other.cell_at(t, 1, 1).unwrap();
    other.insert_text(c, "셀").unwrap();
    let bytes = other.serialize(Format::Hwp).unwrap().0;

    let mut d = open(&blank());
    let p = d.insert_text(START, "앞").unwrap();
    let q = d.split_paragraph(p).unwrap();
    d.insert_text(q, "뒤").unwrap();
    let before = d.body_text();
    let after = d.insert_document(p, &bytes).unwrap();
    let text = d.body_text().join("\n");
    assert!(text.starts_with("앞끼울 글"), "{text:?}");
    assert!(text.trim_end().ends_with("뒤"), "{text:?}");
    // the table came along
    let cell = d.find_all("셀", true);
    assert!(
        matches!(cell.first().map(|r| r.start), Some(Pos::Cell { .. })),
        "{cell:?}"
    );
    // the inserted words keep their bold, the target's words do not
    assert!(d.char_state(START.with_offset(2)).unwrap().bold);
    assert!(!d.char_state(START.with_offset(1)).unwrap().bold);
    // caret lands after the inserted content, before the target's next paragraph
    d.insert_text(after, "|").unwrap();
    assert!(
        d.body_text().join("\n").contains("|"),
        "{:?}",
        d.body_text()
    );
    for fmt in [Format::Hwp, Format::Hwpx] {
        let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
        assert_eq!(back.body_text(), d.body_text(), "{fmt:?}");
        assert!(
            back.char_state(START.with_offset(2)).unwrap().bold,
            "{fmt:?}"
        );
    }
    // inserted 글머리표 paragraphs keep Hwp's "paragraph head" line flag
    let mut list = open(&blank());
    let l0 = list.insert_text(START, "처음").unwrap();
    let l1 = list.split_paragraph(l0).unwrap();
    let l = list.insert_text(l1, "항목").unwrap();
    list.set_list(Range::new(l1, l).unwrap(), ListKind::Bullet, '■')
        .unwrap();
    let l2 = list.split_paragraph(l).unwrap();
    list.insert_text(l2, "끝").unwrap();
    let bytes2 = list.serialize(Format::Hwp).unwrap().0;
    let mut e = open(&blank());
    let x = e.insert_text(START, "앞").unwrap();
    let y = e.split_paragraph(x).unwrap();
    e.insert_text(y, "뒤").unwrap();
    e.insert_document(y, &bytes2).unwrap();
    let item = e
        .find_all("항목", true)
        .first()
        .map(|r| r.start)
        .expect("inserted");
    assert_eq!(e.first_line_has_head_tag(item), Some(true));
    // one undo removes the marker, the next the whole insertion
    d.undo(after).unwrap();
    d.undo(p).unwrap();
    assert_eq!(d.body_text(), before);
    // not inside a table
    let t2 = d.create_table(START, 1, 1).unwrap();
    assert!(d
        .insert_document(d.cell_at(t2, 0, 0).unwrap(), &bytes)
        .is_err());
}

use hwpedit_engine::{CellProps, TableProps};

#[test]
fn cell_and_table_properties() {
    let mut d = open(&blank());
    let t = d.create_table(START, 2, 2).unwrap();
    let c00 = d.cell_at(t, 0, 0).unwrap();
    let c01 = d.cell_at(t, 0, 1).unwrap();
    let c11 = d.cell_at(t, 1, 1).unwrap();
    d.insert_text(c11, "아래").unwrap();
    let y_top = d.caret_rect(c11).unwrap().y;
    // new cells are vertically centred (Hwp's default)
    assert_eq!(d.cell_props(c00).unwrap().vertical_align, 1);
    // 세로 정렬 아래 on one cell only
    d.set_cell_props(c11, c11, Some(2), None, None).unwrap();
    assert_eq!(d.cell_props(c11).unwrap().vertical_align, 2);
    assert_eq!(d.cell_props(c00).unwrap().vertical_align, 1);
    // 안 여백 지정 on the first row: 5mm left
    let x_before = d.caret_rect(c00).unwrap().x;
    let pad_before = d.cell_props(c00).unwrap().margins_mm[0];
    d.set_cell_props(c00, c01, None, Some(Some([5.0, 1.0, 2.0, 2.0])), Some(true))
        .unwrap();
    let p = d.cell_props(c01).unwrap();
    assert!(p.own_margins && p.header, "{p:?}");
    assert!((p.margins_mm[0] - 5.0).abs() < 0.01, "{p:?}");
    // the caret in a cell with a 5mm left margin moves right
    let before = d.cell_props(c11).unwrap();
    assert!(!before.header);
    let x_own = d.caret_rect(c00).unwrap().x;
    let moved_mm = (x_own - x_before) * 25.4 / 96.0;
    assert!(
        (moved_mm - (5.0 - pad_before)).abs() < 0.2,
        "caret moved {moved_mm}mm, margin {pad_before} -> 5mm"
    );
    // 표 속성: 나누지 않음, 제목 줄 반복, 표 안 여백
    let mut tp = d.table_props(c00).unwrap();
    assert_eq!(
        tp.page_break, 2,
        "new tables split at page ends (Hwp default)"
    );
    tp.page_break = 0;
    tp.repeat_header = true;
    tp.margins_mm = [3.0, 3.0, 1.0, 1.0];
    d.set_table_props(c00, &tp).unwrap();
    let got = d.table_props(c11).unwrap();
    assert_eq!((got.page_break, got.repeat_header), (0, true));
    assert!(
        got.margins_mm
            .iter()
            .zip(tp.margins_mm)
            .all(|(a, b)| (a - b).abs() < 0.01),
        "{got:?}"
    );
    // cells without their own margins follow the table's: 3mm left = 11.3px from the cell edge
    let x_table = d.caret_rect(c11).unwrap().x;
    assert!(x_table > 0.0 && y_top >= 0.0);
    for fmt in [Format::Hwp, Format::Hwpx] {
        let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
        let (b00, b11) = (
            back.cell_at(t, 0, 0).unwrap(),
            back.cell_at(t, 1, 1).unwrap(),
        );
        assert_eq!(back.cell_props(b11).unwrap().vertical_align, 2, "{fmt:?}");
        let p = back.cell_props(b00).unwrap();
        assert!(
            p.own_margins && p.header && (p.margins_mm[0] - 5.0).abs() < 0.01,
            "{fmt:?} {p:?}"
        );
        let q = back.table_props(b00).unwrap();
        assert_eq!((q.page_break, q.repeat_header), (0, true), "{fmt:?}");
        assert!((q.margins_mm[0] - 3.0).abs() < 0.01, "{fmt:?} {q:?}");
        assert!(
            (back.caret_rect(b00).unwrap().x - x_own).abs() < 0.6,
            "{fmt:?}"
        );
    }
    let _ = (CellProps { ..before }, TableProps { ..tp });
}

#[test]
fn paragraph_spacing_survives_reopen_in_layout() {
    let mut d = open(&blank());
    let a = d.insert_text(START, "첫째 문단").unwrap();
    let b = d.split_paragraph(a).unwrap();
    d.insert_text(b, "둘째 문단").unwrap();
    d.set_para_format(
        Range {
            start: START,
            end: START,
        },
        &ParaFormat {
            spacing_before_pt: Some(12.0),
            spacing_after_pt: Some(6.0),
            ..Default::default()
        },
    )
    .unwrap();
    let (y1, y2) = (d.caret_rect(START).unwrap().y, d.caret_rect(b).unwrap().y);
    // as Hwp 2022 lays it out (its re-save of the same document, read back): the first
    // paragraph 12pt = 16px lower, the second one line + 6pt (8px) below it
    let mut plain = open(&blank());
    let a2 = plain.insert_text(START, "첫째 문단").unwrap();
    plain.split_paragraph(a2).unwrap();
    let top = plain.caret_rect(START).unwrap().y;
    assert!(
        (y1 - top - 16.0).abs() < 0.6,
        "first paragraph at {y1}, plain {top}"
    );
    assert!((y2 - y1 - 21.33 - 8.0).abs() < 0.6, "{y1} -> {y2}");
    // typing keeps the spacing
    d.insert_text(START.with_offset(2), "가").unwrap();
    assert!((d.caret_rect(START).unwrap().y - y1).abs() < 0.6);
    assert!((d.caret_rect(b).unwrap().y - y2).abs() < 0.6);
    d.undo(START.with_offset(2)).unwrap();
    for fmt in [Format::Hwp, Format::Hwpx] {
        let back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
        let (r1, r2) = (
            back.caret_rect(START).unwrap().y,
            back.caret_rect(b).unwrap().y,
        );
        assert!(
            (r1 - y1).abs() < 0.6 && (r2 - y2).abs() < 0.6,
            "{fmt:?}: in memory {y1},{y2}; reopened {r1},{r2}"
        );
    }
}

#[test]
fn inserted_lists_keep_their_head_flag() {
    // the f06 lists document: 개요 1·2, 문단 번호 two levels, 글머리표, a style
    let mut other = open(&blank());
    let mut at = START;
    let mut paras = Vec::new();
    for (i, text) in [
        "개요 하나",
        "개요 둘",
        "번호 하나",
        "번호 아래 수준",
        "글머리표",
    ]
    .iter()
    .enumerate()
    {
        if i > 0 {
            at = other.split_paragraph(at).unwrap();
        }
        paras.push(at);
        at = other.insert_text(at, text).unwrap();
    }
    let one = |p: Pos| Range::new(p, p).unwrap();
    other.apply_outline(one(paras[0]), 0).unwrap();
    other.apply_outline(one(paras[1]), 1).unwrap();
    other
        .set_list(
            Range::new(paras[2], paras[3]).unwrap(),
            ListKind::Number,
            '●',
        )
        .unwrap();
    other.change_level(one(paras[3]), 1).unwrap();
    other
        .set_list(one(paras[4]), ListKind::Bullet, '■')
        .unwrap();
    let bytes = other.serialize(Format::Hwp).unwrap().0;

    let mut d = open(&blank());
    let a = d.insert_text(START, "앞 글").unwrap();
    let b = d.split_paragraph(a).unwrap();
    d.insert_text(b, "뒤 글").unwrap();
    d.insert_document(b, &bytes).unwrap();
    let back = Document::from_bytes(&d.serialize(Format::Hwp).unwrap().0, Format::Hwp).unwrap();
    for w in ["개요 둘", "번호 하나", "번호 아래 수준", "글머리표"] {
        let at = back.find_all(w, true).first().map(|r| r.start).unwrap();
        assert_eq!(back.first_line_has_head_tag(at), Some(true), "{w}");
    }
}

#[test]
fn tall_equations_stay_inside_their_table_cell() {
    let mut d = open(&blank());
    let t = d.create_table(START, 2, 2).unwrap();
    let c = d.cell_at(t, 1, 0).unwrap();
    let after = d.insert_text(c, "행렬 ").unwrap();
    let script = latex::to_hwp(r"\begin{pmatrix} 1 & 0 \\ 0 & 1 \end{pmatrix}")
        .unwrap()
        .script;
    let eq = d.insert_equation(after, &script).unwrap();
    d.insert_text_after_object(eq, " 끝").unwrap();
    for (label, doc) in [
        ("edited", None),
        ("reopened", Some(Format::Hwp)),
        ("reopened", Some(Format::Hwpx)),
    ] {
        let back;
        let d = match doc {
            None => &d,
            Some(fmt) => {
                back = Document::from_bytes(&d.serialize(fmt).unwrap().0, fmt).unwrap();
                &back
            }
        };
        let cell = d
            .cell_boxes(c, 0)
            .into_iter()
            .find(|b| (b.row, b.col) == (1, 0))
            .unwrap();
        let boxes = d.equation_boxes(0);
        let (_, y, _, h) = *boxes.first().expect("the 수식 is laid out");
        assert!(
            y >= cell.y - 0.5 && y + h <= cell.y + cell.h + 0.5,
            "{label} {doc:?}: 수식 {y}..{} outside the cell {}..{}",
            y + h,
            cell.y,
            cell.y + cell.h
        );
    }
}
