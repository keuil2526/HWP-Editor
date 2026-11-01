//! Documents exercising the ribbon's page and insert commands, for checking in Hwp 2022
//! (`python3 -I tools/hancom.py check OUT OUT/*.hwp OUT/*.hwpx`).
//!
//!   cargo run --release -p hwpedit --example feature_lab -- .tmp/feature-lab

use hwpedit_core::Format;
use hwpedit_engine::latex;
use hwpedit_engine::{
    BorderLine, BorderSides, CharFormat, Document, LineSpacingKind, ListKind, PageBorder, PageHide,
    PageNumber, ParaFormat, Pos, Range, ShapeKind,
};

const START: Pos = Pos::Body {
    section: 0,
    para: 0,
    offset: 0,
};
const PNG: &[u8] = include_bytes!("lab-picture.png");

fn save(d: &Document, out: &str, name: &str) {
    for fmt in [Format::Hwp, Format::Hwpx] {
        let bytes = d.serialize(fmt).expect("serialize").0;
        let back = Document::from_bytes(&bytes, fmt).expect("reopen");
        assert_eq!(
            back.body_text(),
            d.body_text(),
            "{name} {fmt:?}: text changed on reopen"
        );
        std::fs::write(format!("{out}/{name}.{}", fmt.extension()), bytes).unwrap();
    }
    println!("{name}: {} pages", d.page_count());
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| ".tmp/feature-lab".into());
    std::fs::create_dir_all(&out).unwrap();

    // 가로 용지, 여백 20mm
    let mut d = Document::new_blank().unwrap();
    d.insert_text(
        START,
        "가로 용지 시험 문서입니다. 왼쪽·오른쪽 여백은 20mm입니다.",
    )
    .unwrap();
    let mut p = d.page_setup(0).unwrap();
    p.landscape = true;
    p.margin_left_mm = 20.0;
    p.margin_right_mm = 20.0;
    d.set_page_setup(START, &p).unwrap();
    save(&d, &out, "f01-landscape");

    // 2단 + 단 나누기
    let mut d = Document::new_blank().unwrap();
    let a = d.insert_text(START, "첫째 단의 글입니다.").unwrap();
    d.set_columns(START, 2, 8.0).unwrap();
    let b = d.insert_column_break(a).unwrap();
    d.insert_text(b, "둘째 단의 글입니다.").unwrap();
    save(&d, &out, "f02-columns");

    // 그림 + 책갈피 + 하이퍼링크
    let mut d = Document::new_blank().unwrap();
    let a = d.insert_text(START, "한컴 누리집 링크와 그림").unwrap();
    d.insert_hyperlink(
        Range::new(START, START.with_offset(5)).unwrap(),
        "https://www.hancom.com/",
    )
    .unwrap();
    d.add_bookmark(a, "그림앞").unwrap();
    d.insert_picture(a, PNG, "png", (120, 60), 60.0).unwrap();
    save(&d, &out, "f03-picture-link");

    // 2쪽 문서, 둘째 쪽의 쪽 번호·머리말 감추기
    let mut d = Document::new_blank().unwrap();
    let a = d.insert_text(START, "첫째 쪽").unwrap();
    let b = d.insert_page_break(a).unwrap();
    let c = d.insert_text(b, "둘째 쪽 (쪽 번호 감추기)").unwrap();
    d.hide_on_page(
        c,
        PageHide {
            header: true,
            page_number: true,
            ..Default::default()
        },
    )
    .unwrap();
    save(&d, &out, "f04-page-hide");

    // 각주·미주, 머리말·꼬리말
    let mut d = Document::new_blank().unwrap();
    let a = d.insert_text(START, "각주와 미주가 달린 본문").unwrap();
    let b = d.insert_note(a, false, "각주 내용입니다.").unwrap();
    d.insert_note(b, true, "미주 내용입니다.").unwrap();
    d.set_header_footer_text(START, true, "머리말 글").unwrap();
    d.set_header_footer_text(START, false, "꼬리말 글").unwrap();
    save(&d, &out, "f05-notes-header");

    // 개요 1·2, 문단 번호 두 수준, 글머리표 ■, 본문 스타일
    let mut d = Document::new_blank().unwrap();
    let mut at = START;
    let mut paras = Vec::new();
    for (i, text) in [
        "개요 하나",
        "개요 둘",
        "번호 하나",
        "번호 아래 수준",
        "글머리표",
        "본문 스타일",
    ]
    .iter()
    .enumerate()
    {
        if i > 0 {
            at = d.split_paragraph(at).unwrap();
        }
        paras.push(at);
        at = d.insert_text(at, text).unwrap();
    }
    let one = |p: Pos| Range::new(p, p).unwrap();
    d.apply_outline(one(paras[0]), 0).unwrap();
    d.apply_outline(one(paras[1]), 1).unwrap();
    d.set_list(
        Range::new(paras[2], paras[3]).unwrap(),
        ListKind::Number,
        '●',
    )
    .unwrap();
    d.change_level(one(paras[3]), 1).unwrap();
    d.set_list(one(paras[4]), ListKind::Bullet, '■').unwrap();
    let body = d
        .styles()
        .into_iter()
        .find(|s| s.name == "본문")
        .unwrap()
        .id;
    d.apply_style(one(paras[5]), body).unwrap();
    save(&d, &out, "f06-lists-styles");

    // 3x3 표: 왼쪽 위 2x2 노란 음영, 바깥 빨간 이중선 0.4mm, 아래 줄 가운데 세로선 없음
    let mut d = Document::new_blank().unwrap();
    let c = d.create_table(START, 3, 3).unwrap();
    for r in 0..3 {
        for k in 0..3 {
            let at = d.cell_at(c, r, k).unwrap();
            d.insert_text(at, &format!("{}{}", r + 1, k + 1)).unwrap();
        }
    }
    let at = |d: &Document, r, k| d.cell_at(c, r, k).unwrap();
    d.set_cell_fill(at(&d, 0, 0), at(&d, 1, 1), Some("#ffff00"))
        .unwrap();
    let red = BorderLine {
        kind: 8,
        width: 6,
        color: "#ff0000".into(),
    };
    d.set_cell_borders(at(&d, 0, 0), at(&d, 2, 2), BorderSides::Outer, &red)
        .unwrap();
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
    save(&d, &out, "f07-cell-borders");

    // 3쪽: 가운데 아래 쪽 번호 "- n -", 둘째 쪽부터 새 번호 20, 파란 쪽 테두리와 연노랑 배경
    let mut d = Document::new_blank().unwrap();
    let a = d.insert_text(START, "첫째 쪽").unwrap();
    let b = d.insert_page_break(a).unwrap();
    let b2 = d.insert_text(b, "둘째 쪽 (새 번호 20)").unwrap();
    let c = d.insert_page_break(b2).unwrap();
    d.insert_text(c, "셋째 쪽").unwrap();
    d.set_page_number(
        START,
        &PageNumber {
            position: 5,
            format: 0,
            dash: true,
        },
    )
    .unwrap();
    d.set_new_page_number(b, 20).unwrap();
    let blue = BorderLine {
        kind: 1,
        width: 7,
        color: "#0000ff".into(),
    };
    d.set_page_border(
        START,
        &PageBorder {
            borders: [blue.clone(), blue.clone(), blue.clone(), blue],
            fill: Some("#fff2cc".into()),
            page_based: true,
            spacing_mm: [5.0; 4],
            header_inside: false,
            footer_inside: false,
        },
    )
    .unwrap();
    save(&d, &out, "f08-page-number-border");

    // LaTeX로 입력한 수식 8개, 한 문단에 하나씩 (앞에 LaTeX 원문)
    let mut d = Document::new_blank().unwrap();
    let samples = [
        r"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}",
        r"\sum_{k=1}^{n} k^2 = \frac{n(n+1)(2n+1)}{6}",
        r"\int_0^\infty e^{-x^2} dx = \frac{\sqrt{\pi}}{2}",
        r"\lim_{x \to 0} \frac{\sin x}{x} = 1",
        r"\sqrt[3]{8} = 2, \quad \alpha + \beta \le \Gamma",
        r"A = \begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}",
        r"f(x) = \begin{cases} x & x \ge 0 \\ -x & x < 0 \end{cases}",
        r"\left( \frac{a}{b} \right)^{2} \ne \vec{v} \cdot \hat{n}",
    ];
    let mut at = START;
    for (i, tex) in samples.iter().enumerate() {
        if i > 0 {
            at = d.split_paragraph_after_object(at).unwrap();
        }
        let c = latex::to_hwp(tex).unwrap();
        assert!(c.unknown.is_empty(), "{tex}: {:?}", c.unknown);
        let after = d.insert_text(at, &format!("({}) ", i + 1)).unwrap();
        at = d.insert_equation(after, &c.script).unwrap();
        println!("  {tex}  ->  {}", c.script);
    }
    save(&d, &out, "f09-equations");

    // 글자처럼 취급한 도형: 직사각형 30×15, 타원 20×20, 직선 40
    let mut d = Document::new_blank().unwrap();
    let mut at = d.insert_text(START, "도형: ").unwrap();
    for (kind, w, h) in [
        (ShapeKind::Rectangle, 30.0, 15.0),
        (ShapeKind::Ellipse, 20.0, 20.0),
        (ShapeKind::Line, 40.0, 0.0),
    ] {
        at = d.insert_shape(at, kind, w, h, "").unwrap();
        at = d.insert_text_after_object(at, " ").unwrap();
    }
    d.insert_text(at, "끝").unwrap();
    save(&d, &out, "f10-shapes");

    // 표 안 수식: 2×2 표, 셀마다 LaTeX 수식과 글
    let mut d = Document::new_blank().unwrap();
    let c = d.create_table(START, 2, 2).unwrap();
    let cells = [
        ("근의 공식 ", r"x = \frac{-b \pm \sqrt{b^2-4ac}}{2a}"),
        ("합 ", r"\sum_{k=1}^{n} k = \frac{n(n+1)}{2}"),
        ("행렬 ", r"\begin{pmatrix} 1 & 0 \\ 0 & 1 \end{pmatrix}"),
        ("각 ", r"\alpha + \beta = \pi"),
    ];
    for (i, (label, tex)) in cells.iter().enumerate() {
        let at = d.cell_at(c, (i / 2) as u16, (i % 2) as u16).unwrap();
        let after = d.insert_text(at, label).unwrap();
        let script = latex::to_hwp(tex).unwrap().script;
        let eq = d.insert_equation(after, &script).unwrap();
        d.insert_text_after_object(eq, " 끝").unwrap();
    }
    save(&d, &out, "f11-table-equations");

    // 문단 모양: 위 12pt·아래 6pt, 고정 줄 간격 20pt, 최소 18pt, 여백만 4pt,
    // 다음 문단과 함께·문단 보호, 넷째 문단 앞에서 쪽 나눔
    let mut d = Document::new_blank().unwrap();
    let mut at = START;
    let mut paras = Vec::new();
    for (i, text) in [
        "위 12pt, 아래 6pt 간격 문단입니다.",
        "고정 줄 간격 20pt 문단입니다. 줄이 넘어가도록 글을 길게 씁니다. 줄이 넘어가도록 글을 길게 씁니다. 줄이 넘어가도록 글을 길게 씁니다.",
        "최소 줄 간격 18pt, 다음 문단과 함께, 문단 보호.",
        "이 문단 앞에서 쪽을 나눕니다 (여백만 지정 4pt).",
    ]
    .iter()
    .enumerate()
    {
        if i > 0 {
            at = d.split_paragraph(at).unwrap();
        }
        let start = at;
        at = d.insert_text(at, text).unwrap();
        paras.push(Range::new(start, at).unwrap());
    }
    let set = |d: &mut Document, r: Range, f: ParaFormat| d.set_para_format(r, &f).unwrap();
    set(
        &mut d,
        paras[0],
        ParaFormat {
            spacing_before_pt: Some(12.0),
            spacing_after_pt: Some(6.0),
            ..Default::default()
        },
    );
    set(
        &mut d,
        paras[1],
        ParaFormat {
            line_spacing: Some((LineSpacingKind::Fixed, 20.0)),
            ..Default::default()
        },
    );
    set(
        &mut d,
        paras[2],
        ParaFormat {
            line_spacing: Some((LineSpacingKind::Minimum, 18.0)),
            keep_with_next: Some(true),
            keep_lines: Some(true),
            ..Default::default()
        },
    );
    set(
        &mut d,
        paras[3],
        ParaFormat {
            line_spacing: Some((LineSpacingKind::SpaceOnly, 4.0)),
            page_break_before: Some(true),
            ..Default::default()
        },
    );
    save(&d, &out, "f12-paragraph-spacing");

    // 문서 끼워 넣기: f06(목록·스타일)과 f11(표 안 수식)을 글 사이에 끼움
    let mut d = Document::new_blank().unwrap();
    let a = d.insert_text(START, "앞 글").unwrap();
    d.apply_char_format(
        Range::new(START, a).unwrap(),
        &CharFormat {
            italic: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let b = d.split_paragraph(a).unwrap();
    d.insert_text(b, "뒤 글").unwrap();
    for name in ["f11-table-equations", "f06-lists-styles"] {
        let bytes = std::fs::read(format!("{out}/{name}.hwp")).unwrap();
        d.insert_document(b, &bytes).unwrap();
    }
    save(&d, &out, "f13-insert-document");
}
