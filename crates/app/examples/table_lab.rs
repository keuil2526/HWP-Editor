//! Table lab: run the table edits a user makes, through the same engine calls as the
//! window, and after every step check the grid, the rendered cell boxes and a save
//! round trip; render the page so the steps can be looked at.
//! cargo run --release -p hwpedit --example table_lab -- OUT_DIR
#[path = "../src/render.rs"]
mod render;

use hwpedit_core::Format;
use hwpedit_engine::{Document, Pos, Resize};

struct Lab {
    d: Document,
    out: String,
    step: usize,
    problems: Vec<String>,
}

impl Lab {
    fn table_cells(&self, t: Pos) -> Vec<(usize, hwpedit_engine::CellInfo)> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = t
        else {
            return Vec::new();
        };
        (0..)
            .map(|cell| {
                (
                    cell,
                    self.d.cell_info(Pos::Cell {
                        section,
                        parent_para,
                        control,
                        cell,
                        cell_para: 0,
                        offset: 0,
                    }),
                )
            })
            .take_while(|(_, i)| i.is_some())
            .map(|(c, i)| (c, i.unwrap()))
            .collect()
    }

    fn check(&mut self, name: &str, t: Option<Pos>) {
        self.step += 1;
        let tag = format!("{:02}-{name}", self.step);
        let mut bad = Vec::new();
        if let Some(t) = t {
            // grid: every (row, col) covered exactly once
            let cells = self.table_cells(t);
            if let Some((_, first)) = cells.first() {
                let (rows, cols) = (first.rows as usize, first.cols as usize);
                let mut cover = vec![0u8; rows * cols];
                for (c, i) in &cells {
                    for r in i.row..i.row + i.row_span.max(1) {
                        for k in i.col..i.col + i.col_span.max(1) {
                            if (r as usize) < rows && (k as usize) < cols {
                                cover[r as usize * cols + k as usize] += 1;
                            } else {
                                bad.push(format!(
                                    "셀 {c}가 표 밖 ({r},{k}) 차지 (표 {rows}x{cols})"
                                ));
                            }
                        }
                    }
                }
                for (i, n) in cover.iter().enumerate() {
                    if *n != 1 {
                        bad.push(format!("칸 ({},{})를 셀 {n}개가 차지", i / cols, i % cols));
                    }
                }
            } else {
                bad.push("표를 찾을 수 없음".into());
            }
            // geometry: rendered boxes non-empty, non-overlapping, tiling a rectangle
            for page in 0..self.d.page_count() {
                let b = self.d.cell_boxes(t, page);
                if b.is_empty() {
                    continue;
                }
                for x in &b {
                    if x.w < 1.0 || x.h < 1.0 {
                        bad.push(format!(
                            "{}쪽 셀 {} ({},{}) 크기 {:.1}x{:.1}",
                            page + 1,
                            x.cell,
                            x.row,
                            x.col,
                            x.w,
                            x.h
                        ));
                    }
                }
                for (i, p) in b.iter().enumerate() {
                    for q in &b[i + 1..] {
                        let ox = (p.x + p.w).min(q.x + q.w) - p.x.max(q.x);
                        let oy = (p.y + p.h).min(q.y + q.h) - p.y.max(q.y);
                        if ox > 1.0 && oy > 1.0 {
                            bad.push(format!(
                                "{}쪽 셀 {}와 {}가 겹침 ({ox:.1}x{oy:.1})",
                                page + 1,
                                p.cell,
                                q.cell
                            ));
                        }
                    }
                }
                let x0 = b.iter().map(|c| c.x).fold(f64::MAX, f64::min);
                let x1 = b.iter().map(|c| c.x + c.w).fold(f64::MIN, f64::max);
                let y0 = b.iter().map(|c| c.y).fold(f64::MAX, f64::min);
                let y1 = b.iter().map(|c| c.y + c.h).fold(f64::MIN, f64::max);
                let area: f64 = b.iter().map(|c| c.w * c.h).sum();
                let gap = (x1 - x0) * (y1 - y0) - area;
                if gap.abs() > (x1 - x0 + y1 - y0) * 2.0 {
                    bad.push(format!(
                        "{}쪽 셀들이 사각형을 채우지 않음 (빈 넓이 {gap:.0}px²)",
                        page + 1
                    ));
                }
                let (pw, _) = self
                    .d
                    .page_size(page)
                    .map(|s| (s.width, s.height))
                    .unwrap_or((f64::MAX, 0.0));
                if x1 > pw + 0.5 {
                    bad.push(format!(
                        "{}쪽 표가 쪽 오른쪽 밖으로 나감 ({x1:.0} > {pw:.0})",
                        page + 1
                    ));
                }
            }
        }
        // save round trip in both formats
        let text = self.d.all_text();
        for fmt in [Format::Hwp, Format::Hwpx] {
            match self.d.serialize(fmt) {
                Ok(s) => match Document::from_bytes(&s.0, fmt) {
                    Ok(back) => {
                        if back.all_text() != text {
                            bad.push(format!("{fmt:?} 저장 후 다시 열면 텍스트가 다름"));
                        }
                        if back.page_count() != self.d.page_count() {
                            bad.push(format!(
                                "{fmt:?} 저장 후 쪽 수 {} → {}",
                                self.d.page_count(),
                                back.page_count()
                            ));
                        }
                        if let Some(t) = t {
                            let (a, b) = (
                                self.table_cells(t).len(),
                                Lab {
                                    d: back,
                                    out: String::new(),
                                    step: 0,
                                    problems: vec![],
                                }
                                .table_cells(t)
                                .len(),
                            );
                            if a != b {
                                bad.push(format!("{fmt:?} 저장 후 셀 수 {a} → {b}"));
                            }
                        }
                        if fmt == Format::Hwp {
                            std::fs::write(format!("{}/{tag}.hwp", self.out), &s.0).unwrap();
                        }
                    }
                    Err(e) => bad.push(format!("{fmt:?} 저장 결과를 다시 열 수 없음: {e}")),
                },
                Err(e) => bad.push(format!("{fmt:?} 저장 실패: {e}")),
            }
        }
        for p in 0..self.d.page_count() {
            let (w, h, rgba) =
                render::rasterize_rgba(&self.d.render_page_svg(p).unwrap(), 1.0).unwrap();
            let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
            ppm.extend(rgba.chunks_exact(4).flat_map(|px| [px[0], px[1], px[2]]));
            std::fs::write(format!("{}/{tag}-p{}.ppm", self.out, p + 1), ppm).unwrap();
        }
        println!(
            "{tag}: {}",
            if bad.is_empty() {
                "ok".to_string()
            } else {
                bad.join(" | ")
            }
        );
        self.problems
            .extend(bad.into_iter().map(|b| format!("{tag}: {b}")));
    }
}

fn type_text(d: &mut Document, mut p: Pos, s: &str) -> Pos {
    for ch in s.chars() {
        p = d.insert_text(p, &ch.to_string()).unwrap();
    }
    p
}

fn main() {
    let out = std::env::args().nth(1).expect("OUT_DIR");
    std::fs::create_dir_all(&out).unwrap();
    let mut lab = Lab {
        d: Document::new_blank().unwrap(),
        out,
        step: 0,
        problems: Vec::new(),
    };
    let start = Pos::Body {
        section: 0,
        para: 0,
        offset: 0,
    };

    // a heading line, then a table in a new empty paragraph (as the grid picker does)
    let p = type_text(&mut lab.d, start, "표 시험 문서");
    let p = lab.d.split_paragraph(p).unwrap();
    let t = lab.d.create_table(p, 3, 3).unwrap();
    lab.check("create-3x3", Some(t));

    // fill cells with Tab, like a user
    let mut c = t;
    for (i, s) in [
        "이름",
        "학번",
        "비고",
        "홍길동",
        "20261234",
        "",
        "김철수",
        "20265678",
        "결석",
    ]
    .iter()
    .enumerate()
    {
        c = type_text(&mut lab.d, c, s);
        if i < 8 {
            c = lab.d.tab_forward(c).unwrap();
        }
    }
    lab.check("fill-tab", Some(t));

    // Tab in the last cell adds a row
    c = lab.d.tab_forward(c).unwrap();
    type_text(&mut lab.d, c, "새 줄");
    lab.check("tab-new-row", Some(t));

    // long text that wraps, then Enter inside a cell
    let long = lab.d.cell_at(t, 1, 2).unwrap();
    let e = type_text(
        &mut lab.d,
        long,
        "아주 긴 비고 내용이 들어가서 칸 너비를 넘으면 줄이 바뀌어야 합니다",
    );
    let e = lab.d.split_paragraph(e).unwrap();
    type_text(&mut lab.d, e, "둘째 문단");
    lab.check("wrap-enter", Some(t));

    // Backspace at the start of the second cell paragraph merges it back
    let e2 = match e {
        Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            ..
        } => Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            cell_para: 1,
            offset: 0,
        },
        p => p,
    };
    lab.d.merge_with_previous(e2).unwrap();
    lab.check("cell-backspace", Some(t));

    // structure edits
    let a = lab.d.cell_at(t, 0, 0).unwrap();
    lab.d.insert_col(a, true).unwrap();
    lab.check("insert-col", Some(t));
    let a = lab.d.cell_at(t, 1, 0).unwrap();
    lab.d.insert_row(a, false).unwrap();
    lab.check("insert-row-above", Some(t));
    let (a, b) = (
        lab.d.cell_at(t, 0, 0).unwrap(),
        lab.d.cell_at(t, 0, 1).unwrap(),
    );
    lab.d.merge_cells(a, b).unwrap();
    lab.check("merge-row0", Some(t));
    let (a, b) = (
        lab.d.cell_at(t, 2, 2).unwrap(),
        lab.d.cell_at(t, 3, 2).unwrap(),
    );
    lab.d.merge_cells(a, b).unwrap();
    lab.check("merge-vertical", Some(t));
    let a = lab.d.cell_at(t, 0, 0).unwrap();
    let _ = lab.d.split_cell(a).map_err(|e| println!("  split: {e}"));
    lab.check("split", Some(t));
    let a = lab.d.cell_at(t, 1, 1).unwrap();
    let _ = lab.d.delete_row(a);
    lab.check("delete-row", Some(t));
    let a = lab.d.cell_at(t, 0, 3).unwrap_or(t);
    let _ = lab.d.delete_col(a);
    lab.check("delete-col", Some(t));

    // sizes
    let a = lab.d.cell_at(t, 0, 0).unwrap();
    let _ = lab
        .d
        .resize_table(a, false, 10, Resize::Table)
        .map_err(|e| println!("  resize col: {e}"));
    lab.check("widen-col", Some(t));
    let _ = lab
        .d
        .resize_table(a, true, 6, Resize::Table)
        .map_err(|e| println!("  resize row: {e}"));
    lab.check("taller-row", Some(t));
    let _ = lab
        .d
        .resize_table(a, false, 8, Resize::Border)
        .map_err(|e| println!("  border: {e}"));
    lab.check("move-border", Some(t));
    let _ = lab
        .d
        .scale_table(a, 1.4, 1.0)
        .map_err(|e| println!("  scale: {e}"));
    lab.check("scale-wide", Some(t));
    let _ = lab
        .d
        .equalize_columns(a)
        .map_err(|e| println!("  equalize: {e}"));
    lab.check("equal-cols", Some(t));

    // text after the table, then a second table at the end of a text paragraph
    let after = match t {
        Pos::Cell {
            section,
            parent_para,
            ..
        } => Pos::Body {
            section,
            para: parent_para + 1,
            offset: 0,
        },
        p => p,
    };
    if lab.d.para_count(after)
        > match after {
            Pos::Body { para, .. } => para,
            _ => 0,
        }
    {
        let q = type_text(&mut lab.d, after, "표 아래 글");
        lab.check("text-after-table", Some(t));
        let t2 = lab.d.create_table(q, 2, 2);
        match t2 {
            Ok(t2) => {
                type_text(&mut lab.d, t2, "둘째 표");
                lab.check("table-mid-paragraph", Some(t2));
            }
            Err(e) => println!("  create at end of text: {e}"),
        }
    } else {
        lab.problems
            .push("표 뒤에 문단이 없어 표 아래에 글을 쓸 수 없음".into());
        println!("no paragraph after the table");
    }

    // a big table that must split across pages
    let end = Pos::Body {
        section: 0,
        para: lab.d.para_count(start) - 1,
        offset: 0,
    };
    let end = Pos::Body {
        section: 0,
        para: lab.d.para_count(start) - 1,
        offset: lab.d.para_len(end),
    };
    let end = lab.d.split_paragraph(end).unwrap();
    let t3 = lab.d.create_table(end, 40, 2).unwrap();
    let mut c = t3;
    for i in 0..79 {
        c = type_text(&mut lab.d, c, &format!("{i}번"));
        c = lab.d.tab_forward(c).unwrap();
    }
    lab.check("table-across-pages", Some(t3));

    // undo everything
    let mut n = 0;
    while lab.d.can_undo() && n < 300 {
        let _ = lab.d.undo(start);
        n += 1;
    }
    lab.check("undo-all", None);
    if lab.d.edited() && n < 98 {
        lab.problems
            .push(format!("실행 취소 {n}번 뒤에도 수정됨 상태"));
    }

    // where the caret can be when 표 만들기 runs
    let cases: [(&str, &[&str], usize, Option<usize>); 5] = [
        ("end-of-line", &["첫 줄 글자"], 0, None),
        ("middle-of-line", &["앞부분뒤부분"], 0, Some(3)),
        ("start-of-line", &["글자 있는 줄"], 0, Some(0)),
        ("empty-line-between", &["위 글", "", "아래 글"], 1, None),
        ("last-of-three", &["하나", "둘", "셋"], 2, None),
    ];
    for (name, lines, para, offset) in cases {
        let mut d = Document::new_blank().unwrap();
        let mut p = start;
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                p = d.split_paragraph(p).unwrap();
            }
            p = type_text(&mut d, p, l);
        }
        let at = Pos::Body {
            section: 0,
            para,
            offset: offset.unwrap_or(lines[para].chars().count()),
        };
        let before: Vec<String> = d.body_text();
        let mut case = Lab {
            d,
            out: lab.out.clone(),
            step: 0,
            problems: Vec::new(),
        };
        match case.d.create_table(at, 2, 3) {
            Ok(t) => {
                type_text(&mut case.d, t, "표");
                case.step = 50 + lab.step;
                case.check(&format!("case-{name}"), Some(t));
                println!("    문단: {:?} -> {:?}", before, case.d.body_text());
            }
            Err(e) => case
                .problems
                .push(format!("case-{name}: 표를 만들 수 없음: {e}")),
        }
        lab.step += 1;
        lab.problems.extend(case.problems);
    }

    // a second table under a first one
    for (name, text, rows) in [
        ("second-after-text", "표 아래 글", 2u16),
        ("second-empty-line", "", 2),
        ("second-after-tall", "표 아래 글", 6),
    ] {
        let mut d = Document::new_blank().unwrap();
        let p = type_text(&mut d, start, "제목");
        let p = d.split_paragraph(p).unwrap();
        let t1 = d.create_table(p, rows, 3).unwrap();
        type_text(&mut d, t1, "첫 표");
        let Pos::Cell { parent_para, .. } = t1 else {
            unreachable!()
        };
        let below = Pos::Body {
            section: 0,
            para: parent_para + 1,
            offset: 0,
        };
        let q = type_text(&mut d, below, text);
        let mut case = Lab {
            d,
            out: lab.out.clone(),
            step: 80 + lab.step,
            problems: Vec::new(),
        };
        let t2 = case.d.create_table(q, 2, 2).unwrap();
        type_text(&mut case.d, t2, "둘째 표");
        case.check(&format!("case-{name}"), Some(t2));
        println!("    문단: {:?}", case.d.body_text());
        lab.step += 1;
        lab.problems.extend(case.problems);
    }

    // edge keys inside a table
    {
        let mut d = Document::new_blank().unwrap();
        let t = d.create_table(start, 2, 2).unwrap();
        let mut case = Lab {
            d,
            out: lab.out.clone(),
            step: 120,
            problems: Vec::new(),
        };
        let say = |case: &mut Lab, what: &str, r: Result<String, String>| match r {
            Ok(m) => println!("  {what}: {m}"),
            Err(e) => case.problems.push(format!("{what}: {e}")),
        };
        let before = case.d.all_text();
        let r = case.d.merge_with_previous(t);
        let r = match r {
            Ok(None) if case.d.all_text() == before => Ok("무시됨".into()),
            Ok(x) => Err(format!("첫 칸 맨 앞 Backspace가 무언가를 바꿈: {x:?}")),
            Err(e) => Ok(format!("거부: {e}")),
        };
        say(&mut case, "첫 칸 맨 앞 Backspace", r);
        let last = case.d.cell_at(t, 1, 1).unwrap();
        let n0 = case.d.para_count(start);
        let r = case.d.delete_text(last, 1);
        let r = if case.d.para_count(start) == n0 && case.d.cell_info(last).is_some() {
            Ok(format!("{r:?}"))
        } else {
            Err(format!("마지막 칸 끝 Delete가 표 밖을 지움: {r:?}"))
        };
        say(&mut case, "마지막 칸 끝 Delete", r);
        let r = case
            .d
            .insert_page_break(last)
            .map(|_| "쪽 나누기가 들어감".to_string())
            .map_err(|e| e.to_string());
        println!("  셀 안 Ctrl+Enter: {r:?}");
        let r = case
            .d
            .create_table(last, 2, 2)
            .map(|_| "표 안에 표가 만들어짐".to_string())
            .map_err(|e| e.to_string());
        println!("  셀 안 표 만들기: {r:?}");
        let c = case.d.cell_at(t, 0, 1).unwrap();
        let after = case.d.paste_text(c, None, "붙인 첫 줄\n둘째 줄\n셋째 줄");
        match after {
            Ok(p) => {
                let Pos::Cell { cell_para, .. } = p else {
                    unreachable!()
                };
                println!("  셀에 여러 줄 붙이기: 캐럿 문단 {cell_para}");
            }
            Err(e) => case.problems.push(format!("셀에 여러 줄 붙이기: {e}")),
        }
        case.check("edge-keys", Some(t));
        // caret stays inside its cell for every cell
        for (cell, _) in case.table_cells(t) {
            let Pos::Cell {
                section,
                parent_para,
                control,
                ..
            } = t
            else {
                unreachable!()
            };
            let p = Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                cell_para: 0,
                offset: 0,
            };
            match case.d.caret_rect(p) {
                Ok(r) => {
                    if let Some((x, y, w, h)) = r.cell {
                        if r.x < x - 1.0
                            || r.x > x + w + 1.0
                            || r.y < y - 1.0
                            || r.y + r.height > y + h + 1.0
                        {
                            case.problems.push(format!("셀 {cell} 캐럿 ({:.0},{:.0},h{:.0})이 셀 ({x:.0},{y:.0},{w:.0}x{h:.0}) 밖", r.x, r.y, r.height));
                        }
                    }
                }
                Err(e) => case.problems.push(format!("셀 {cell} 캐럿 위치 오류: {e}")),
            }
        }
        // undo the whole thing back to a blank document
        while case.d.can_undo() {
            let _ = case.d.undo(start);
        }
        if case.d.edited() || case.d.cell_info(t).is_some() {
            case.problems
                .push("표 만들기까지 되돌려도 원래 상태가 아님".into());
        }
        lab.problems.extend(case.problems);
    }

    println!("\n문제 {}건", lab.problems.len());
    for p in &lab.problems {
        println!("- {p}");
    }
}
