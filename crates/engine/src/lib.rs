//! Typed adapter over rhwp's `DocumentCore`.
//!
//! rhwp's native API speaks JSON strings; this crate turns them into Rust
//! types, tracks whether the user changed anything, and refuses serializer
//! output that rhwp itself reports as lossy.

pub mod loss;

use hwpedit_core::Format;
pub mod latex;

use rhwp::document_core::DocumentCore;
use rhwp::error::HwpError;
use serde_json::Value;

#[derive(Debug)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EngineError {}

fn err(e: impl std::fmt::Display) -> EngineError {
    EngineError(e.to_string())
}

/// A caret position the engine can edit at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pos {
    Body {
        section: usize,
        para: usize,
        offset: usize,
    },
    /// Paragraph inside a (non-nested) table cell.
    Cell {
        section: usize,
        parent_para: usize,
        control: usize,
        cell: usize,
        cell_para: usize,
        offset: usize,
    },
}

impl Pos {
    pub fn offset(&self) -> usize {
        match *self {
            Pos::Body { offset, .. } | Pos::Cell { offset, .. } => offset,
        }
    }

    pub fn with_offset(self, offset: usize) -> Pos {
        match self {
            Pos::Body { section, para, .. } => Pos::Body {
                section,
                para,
                offset,
            },
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                cell_para,
                ..
            } => Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                cell_para,
                offset,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaretRect {
    pub page: u32,
    pub x: f64,
    pub y: f64,
    pub height: f64,
    /// For a caret in a table cell: the cell's box on the page (x, y, w, h).
    pub cell: Option<(f64, f64, f64, f64)>,
}

/// Position and span of a table cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellInfo {
    pub row: u16,
    pub col: u16,
    pub row_span: u16,
    pub col_span: u16,
    pub rows: u16,
    pub cols: u16,
}

/// A table cell's box on a page (page pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellBox {
    pub cell: usize,
    pub row: u16,
    pub col: u16,
    pub row_span: u16,
    pub col_span: u16,
    pub page: u32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// How a table resize treats the rest of the table (Hwp's 셀 크기 조절 modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resize {
    /// The caret's whole column/row grows or shrinks; the table grows or shrinks with it.
    Table,
    /// The border to the right/below moves; the neighbour gives or takes the space.
    Border,
}

/// One resize step in HWPUNIT (Hwp's measured step, ~1 mm).
pub const RESIZE_STEP_HWPUNIT: f64 = 283.0;
/// HWPUNIT per page pixel at 96 dpi.
pub const HWPUNIT_PER_PX: f64 = 75.0;

/// True if both positions are in cells of the same table.
pub fn same_table(a: Pos, b: Pos) -> bool {
    matches!((a, b), (
        Pos::Cell { section: s1, parent_para: p1, control: c1, .. },
        Pos::Cell { section: s2, parent_para: p2, control: c2, .. },
    ) if (s1, p1, c1) == (s2, p2, c2))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize {
    pub width: f64,
    pub height: f64,
    /// Body (text area) margins in page pixels: left, top, right, bottom.
    pub margins: (f64, f64, f64, f64),
}

/// Where a click landed.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Editable(Pos),
    /// Inside something this version does not edit (nested table, text box, ...).
    ReadOnly(String),
    Nothing,
}

/// Kind of edit, for coalescing consecutive keystrokes into one undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Typing,
    Deleting,
    Other,
}

struct Checkpoint {
    snapshot: u32,
    caret: Pos,
    gen: u64,
    removed: loss::Inventory,
}

/// A selection inside one container (body section or one table cell).
/// `start <= end` after [`Range::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: Pos,
    pub end: Pos,
}

/// Paragraph index within its container (body paragraph or cell paragraph).
fn para_of(p: Pos) -> usize {
    match p {
        Pos::Body { para, .. } => para,
        Pos::Cell { cell_para, .. } => cell_para,
    }
}

fn with_para(p: Pos, i: usize) -> Pos {
    match p {
        Pos::Body {
            section, offset, ..
        } => Pos::Body {
            section,
            para: i,
            offset,
        },
        Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            offset,
            ..
        } => Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            cell_para: i,
            offset,
        },
    }
}

/// True if both positions are in the same body section or the same cell.
fn same_container(a: Pos, b: Pos) -> bool {
    with_para(a.with_offset(0), 0) == with_para(b.with_offset(0), 0)
}

impl Range {
    /// `None` if the two ends are in different containers.
    pub fn new(a: Pos, b: Pos) -> Option<Range> {
        if !same_container(a, b) {
            return None;
        }
        let key = |p: Pos| (para_of(p), p.offset());
        Some(if key(a) <= key(b) {
            Range { start: a, end: b }
        } else {
            Range { start: b, end: a }
        })
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Character properties to apply; `None` leaves a property unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CharFormat {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub size_pt: Option<f64>,
    /// `#rrggbb`
    pub color: Option<String>,
    pub font: Option<String>,
}

/// Character properties at a position, for the toolbar.
#[derive(Debug, Clone, PartialEq)]
pub struct CharState {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub size_pt: f64,
    pub color: String,
    pub font: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Justify,
    /// 배분 정렬: spreads every line, the last one too.
    Distribute,
    /// 나눔 정렬: spreads the space between words only.
    Split,
}

impl Align {
    fn key(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::Justify => "justify",
            Align::Distribute => "distribute",
            Align::Split => "split",
        }
    }
}

/// Paragraph properties to apply; `None` leaves a property unchanged.
/// Lengths are in points, as in Hancom's paragraph dialog.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParaFormat {
    /// Line spacing in percent of the font size (rhwp "Percent" type).
    pub line_spacing_pct: Option<i32>,
    pub margin_left_pt: Option<f64>,
    pub margin_right_pt: Option<f64>,
    /// First-line indent; negative = hanging indent (내어쓰기).
    pub indent_pt: Option<f64>,
    /// 줄 간격 of any kind (overrides `line_spacing_pct`): percent, or points for the others.
    pub line_spacing: Option<(LineSpacingKind, f64)>,
    /// 문단 위·아래 간격.
    pub spacing_before_pt: Option<f64>,
    pub spacing_after_pt: Option<f64>,
    /// 줄 나눔: 외톨이줄 보호, 다음 문단과 함께, 문단 보호, 문단 앞에서 항상 쪽 나눔.
    pub widow_orphan: Option<bool>,
    pub keep_with_next: Option<bool>,
    pub keep_lines: Option<bool>,
    pub page_break_before: Option<bool>,
}

/// Hwp's 줄 간격 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineSpacingKind {
    /// 글자에 따라 (%)
    Percent,
    /// 고정 값 (pt)
    Fixed,
    /// 여백만 지정 (pt)
    SpaceOnly,
    /// 최소 (pt)
    Minimum,
}

impl LineSpacingKind {
    fn rhwp(self) -> &'static str {
        match self {
            LineSpacingKind::Percent => "Percent",
            LineSpacingKind::Fixed => "Fixed",
            LineSpacingKind::SpaceOnly => "SpaceOnly",
            LineSpacingKind::Minimum => "Minimum",
        }
    }
}

/// Paragraph properties at a position, for the toolbar.
#[derive(Debug, Clone, PartialEq)]
pub struct ParaState {
    pub align: Align,
    /// `None` when the spacing is not a percentage (fixed, minimum, ...).
    pub line_spacing_pct: Option<f64>,
    pub margin_left_pt: f64,
    pub margin_right_pt: f64,
    pub indent_pt: f64,
    /// 줄 간격 종류와 값 (percent, or points for the other kinds).
    pub line_spacing: (LineSpacingKind, f64),
    pub spacing_before_pt: f64,
    pub spacing_after_pt: f64,
    pub widow_orphan: bool,
    pub keep_with_next: bool,
    pub keep_lines: bool,
    pub page_break_before: bool,
}

/// Hwp's 셀 속성 (셀 탭): 세로 정렬, 안 여백 지정, 제목 셀.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellProps {
    /// 0 위, 1 가운데, 2 아래.
    pub vertical_align: u8,
    /// 안 여백 지정: the cell's own margins instead of the table's.
    pub own_margins: bool,
    /// 왼쪽, 오른쪽, 위쪽, 아래쪽 (mm).
    pub margins_mm: [f64; 4],
    /// 제목 셀 (repeated on each page with 제목 줄 반복).
    pub header: bool,
}

/// Hwp's 표 속성 (표 탭) parts this version edits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TableProps {
    /// 쪽 경계에서: 0 나누지 않음, 1 셀 단위로 나눔, 2 나눔 (rhwp `TablePageBreak`).
    pub page_break: u8,
    /// 제목 줄 자동 반복.
    pub repeat_header: bool,
    /// 모든 셀의 안 여백: 왼쪽, 오른쪽, 위쪽, 아래쪽 (mm).
    pub margins_mm: [f64; 4],
}

/// One border line: Hwp's 테두리 종류 (`kind`: 0 none, 1 solid, 2 dash, 3 dot, …, 8 double),
/// 테두리 굵기 (`width`: index into 0.1, 0.12, 0.15, 0.2, 0.25, 0.3, 0.4, 0.5, 0.6, 0.7, 1, 1.5,
/// 2, 3, 4, 5 mm) and 테두리 색 ("#rrggbb").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorderLine {
    pub kind: u8,
    pub width: u8,
    pub color: String,
}

/// Which edges of a block of cells Hwp's 테두리˅ draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderSides {
    All,
    Outer,
    Inner,
    Left,
    Right,
    Top,
    Bottom,
    InnerHorizontal,
    InnerVertical,
}

/// A cell's borders (left, right, top, bottom) and 셀 음영 ("#rrggbb").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellLook {
    pub borders: [BorderLine; 4],
    pub fill: Option<String>,
}

/// A style in the document's style list (Hwp's 스타일).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleInfo {
    pub id: usize,
    pub name: String,
    /// 글자 스타일 (applies to characters, not paragraphs).
    pub is_char: bool,
}

/// A paragraph's head (Hwp's 문단 머리).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    None,
    Outline,
    Number,
    Bullet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListState {
    pub kind: ListKind,
    /// 0-based level (Hwp's 수준 1 = 0).
    pub level: u8,
}

enum LevelChange {
    Level(u8),
    Style(usize),
}

/// apply_para_format takes HWPUNIT x2 (7200 = half an inch); 1 pt = 100 HWPUNIT.
const PARA_UNITS_PER_PT: f64 = 200.0;
/// get_para_properties reports lengths in 96-dpi pixels.
const PT_PER_PX: f64 = 72.0 / 96.0;
/// HWPUNIT is 1/7200 inch.
const HU_PER_MM: f64 = 7200.0 / 25.4;

/// A section's paper (Hwp's 편집 용지). Width and height are the paper as stored;
/// `landscape` turns it sideways, as Hwp's 가로/세로 does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSetup {
    pub width_mm: f64,
    pub height_mm: f64,
    pub landscape: bool,
    pub margin_left_mm: f64,
    pub margin_right_mm: f64,
    pub margin_top_mm: f64,
    pub margin_bottom_mm: f64,
    pub margin_header_mm: f64,
    pub margin_footer_mm: f64,
    pub margin_gutter_mm: f64,
}

/// Hwp's 도형 kinds offered by 입력 › 도형, and 글상자.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    Line,
    TextBox,
}

/// Hwp's 쪽 테두리/배경 (양쪽): borders (left, right, top, bottom), 배경 colour, 위치
/// (쪽 기준 or 종이 기준) with its distances, 머리말/꼬리말 포함.
#[derive(Debug, Clone, PartialEq)]
pub struct PageBorder {
    pub borders: [BorderLine; 4],
    pub fill: Option<String>,
    pub page_based: bool,
    pub spacing_mm: [f64; 4],
    pub header_inside: bool,
    pub footer_inside: bool,
}

/// Hwp's 쪽 번호 매기기. `position`: 0 없음, 1 왼쪽 위, 2 가운데 위, 3 오른쪽 위,
/// 4 왼쪽 아래, 5 가운데 아래, 6 오른쪽 아래, 7 바깥쪽 위, 8 바깥쪽 아래, 9 안쪽 위,
/// 10 안쪽 아래. `format`: Hwp's number shape code (0 1 2 3, 1 ①②③, 2 I II, 3 i ii,
/// 4 A B, 5 a b, 8 가 나 다, …). `dash`: 줄표 넣기 ("- 1 -").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageNumber {
    pub position: u8,
    pub format: u8,
    pub dash: bool,
}

/// What 현재 쪽만 감추기 hides on one page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageHide {
    pub header: bool,
    pub footer: bool,
    pub master_page: bool,
    pub border: bool,
    pub fill: bool,
    pub page_number: bool,
}

fn section_of(p: Pos) -> usize {
    match p {
        Pos::Body { section, .. } | Pos::Cell { section, .. } => section,
    }
}

/// Selection highlight rectangle in page pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelRect {
    pub page: u32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

pub struct Document {
    core: DocumentCore,
    format: Format,
    undo: Vec<Checkpoint>,
    redo: Vec<Checkpoint>,
    /// Generation of the current model; bumped by every successful edit.
    gen: u64,
    next_gen: u64,
    /// Generation that matches the bytes on disk.
    saved_gen: u64,
    last_step: Option<Step>,
    /// Records/elements the user deleted on purpose since the last save (tables with
    /// their pictures...), so the save-time loss check does not report them.
    removed: loss::Inventory,
    /// Selection rectangles of whole paragraphs (keyed by the paragraph's start) for
    /// generation `.0`: a drag recomputes only the paragraphs at its two ends.
    sel_cache: std::cell::RefCell<(u64, std::collections::HashMap<Pos, Vec<SelRect>>)>,
}

/// Serialized bytes that rhwp reported as complete (no content loss).
pub struct Serialized(pub Vec<u8>);

/// rhwp evicts its oldest snapshot past this many; stay one below.
const MAX_UNDO: usize = DocumentCore::MAX_SNAPSHOTS - 1;

impl Document {
    pub fn from_bytes(bytes: &[u8], format: Format) -> Result<Self, EngineError> {
        let core = DocumentCore::from_bytes(bytes).map_err(err)?;
        Ok(Document {
            core,
            format,
            undo: Vec::new(),
            redo: Vec::new(),
            gen: 0,
            next_gen: 1,
            saved_gen: 0,
            last_step: None,
            removed: loss::Inventory::new(),
            sel_cache: Default::default(),
        })
    }

    /// A new document from rhwp's Hancom-made blank template.
    pub fn new_blank() -> Result<Self, EngineError> {
        let mut core = DocumentCore::new_empty();
        core.create_blank_document_native().map_err(err)?;
        Ok(Document {
            core,
            format: Format::Hwp,
            undo: Vec::new(),
            redo: Vec::new(),
            gen: 0,
            next_gen: 1,
            saved_gen: 0,
            last_step: None,
            removed: loss::Inventory::new(),
            sel_cache: Default::default(),
        })
    }

    pub fn format(&self) -> Format {
        self.format
    }

    /// The format a "save as" chose for a new document.
    pub fn set_format(&mut self, format: Format) {
        self.format = format;
    }

    /// True if the model differs from what is on disk. Undoing back to the
    /// saved state makes this false again, so an unchanged file is not rewritten.
    pub fn edited(&self) -> bool {
        self.gen != self.saved_gen
    }

    /// Call after the serialized bytes reached disk: the file now matches the model.
    pub fn mark_saved(&mut self) {
        self.saved_gen = self.gen;
        self.last_step = None;
        // the file on disk is the new baseline; older allowances would hide real losses
        self.removed.clear();
        for c in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            c.removed.clear();
        }
    }

    /// What deliberate deletions removed since the last save; see [`loss::detect_losses_allowing`].
    pub fn removed_objects(&self) -> &loss::Inventory {
        &self.removed
    }

    /// Run a deleting edit and remember what it removed, measured by serializing
    /// before and after. If serializing fails nothing is remembered, so the save
    /// check stays strict.
    fn counting_removals<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let count = |d: &Self| {
            d.serialize(d.format)
                .ok()
                .and_then(|s| loss::inventory(&s.0, d.format).ok())
        };
        let before = count(self);
        let v = f(self)?;
        if let (Some(b), Some(a)) = (before, count(self)) {
            for (k, n) in loss::removed_between(&b, &a) {
                *self.removed.entry(k).or_insert(0) += n;
            }
        }
        Ok(v)
    }

    // ------------------------------------------------------------------ history

    /// Run one edit. Takes an undo snapshot unless this continues a run of the same
    /// coalescable `step`; rolls the snapshot back if the edit fails.
    fn mutate<T>(
        &mut self,
        step: Step,
        caret: Pos,
        f: impl FnOnce(&mut DocumentCore) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let coalesce = step != Step::Other && self.last_step == Some(step) && !self.undo.is_empty();
        if !coalesce {
            let snapshot = self.core.save_snapshot_native();
            self.undo.push(Checkpoint {
                snapshot,
                caret,
                gen: self.gen,
                removed: self.removed.clone(),
            });
            if self.undo.len() > MAX_UNDO {
                let old = self.undo.remove(0);
                self.core.discard_snapshot_native(old.snapshot);
            }
        }
        let host = self.table_host(caret);
        match f(&mut self.core) {
            Ok(v) => {
                // an edit inside a table can change its height: keep the stored height and
                // the positions of what follows in step (same undo step)
                if host.is_some() && self.table_host(caret) == host {
                    self.sync_table_flow(caret);
                }
                // reflow drops the 문단 머리 line flag; restore it on the edited paragraph
                // and the one an Enter may have split off
                sync_head_tag(&mut self.core, caret, para_of(caret));
                sync_head_tag(&mut self.core, caret, para_of(caret) + 1);
                for c in self.redo.drain(..) {
                    self.core.discard_snapshot_native(c.snapshot);
                }
                self.gen = self.next_gen;
                self.next_gen += 1;
                self.last_step = Some(step);
                Ok(v)
            }
            Err(e) => {
                if !coalesce {
                    if let Some(c) = self.undo.pop() {
                        let _ = self.core.restore_snapshot_native(c.snapshot);
                        self.core.discard_snapshot_native(c.snapshot);
                    }
                }
                self.last_step = None;
                Err(e)
            }
        }
    }

    /// (section, paragraph, control, controls in that paragraph) of the table at `pos`.
    fn table_host(&self, pos: Pos) -> Option<(usize, usize, usize, usize)> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = pos
        else {
            return None;
        };
        self.table(pos)?;
        let n = self.core.document().sections[section].paragraphs[parent_para]
            .controls
            .len();
        Some((section, parent_para, control, n))
    }

    /// See rhwp `sync_table_flow_native`: a table created or edited here gets its stored
    /// height and the vertical positions after it updated, so the next table is not saved
    /// on top of it.
    fn sync_table_flow(&mut self, pos: Pos) {
        if let Some((section, para, control, _)) = self.table_host(pos) {
            let _ = self.core.sync_table_flow_native(section, para, control);
        }
    }

    /// End the current typing/deleting run (caret moved, focus changed, ...).
    pub fn break_coalescing(&mut self) {
        self.last_step = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undo one step; returns where the caret was before that step.
    pub fn undo(&mut self, caret: Pos) -> Result<Option<Pos>, EngineError> {
        self.swap_history(caret, true)
    }

    pub fn redo(&mut self, caret: Pos) -> Result<Option<Pos>, EngineError> {
        self.swap_history(caret, false)
    }

    fn swap_history(&mut self, caret: Pos, undo: bool) -> Result<Option<Pos>, EngineError> {
        let (from, to) = if undo {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let Some(c) = from.pop() else { return Ok(None) };
        let here = self.core.save_snapshot_native();
        to.push(Checkpoint {
            snapshot: here,
            caret,
            gen: self.gen,
            removed: self.removed.clone(),
        });
        self.core.restore_snapshot_native(c.snapshot).map_err(err)?;
        self.core.discard_snapshot_native(c.snapshot);
        self.gen = c.gen;
        self.removed = c.removed;
        self.last_step = None;
        Ok(Some(c.caret))
    }

    // ------------------------------------------------------------------ queries

    pub fn page_count(&self) -> u32 {
        self.core.page_count()
    }

    pub fn page_size(&self, page: u32) -> Result<PageSize, EngineError> {
        let v = json(&self.core.get_page_info_native(page).map_err(err)?)?;
        let m = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        // the text area starts below the header area and ends above the footer area
        let top = m("marginTop") + m("marginHeader");
        let bottom = m("marginBottom") + m("marginFooter");
        Ok(PageSize {
            width: f(&v, "width")?,
            height: f(&v, "height")?,
            margins: (m("marginLeft"), top, m("marginRight"), bottom),
        })
    }

    /// 보기 › 표시/숨기기: 문단 부호 and 투명 선. Screen only; the file is not changed.
    pub fn set_view_marks(&mut self, paragraph_marks: bool, transparent_lines: bool) {
        self.core
            .set_view_marks_native(paragraph_marks, transparent_lines);
    }

    pub fn render_page_svg(&self, page: u32) -> Result<String, EngineError> {
        self.core.render_page_svg_native(page).map_err(err)
    }

    pub fn hit_test(&self, page: u32, x: f64, y: f64) -> Result<Hit, EngineError> {
        self.hit_test_at_object(page, x, y).map(|h| h.0)
    }

    /// [`Self::hit_test`], also saying whether the point is right after an inline object
    /// (수식, 각주 …): typing there goes after it ([`Self::insert_text_after_object`]).
    pub fn hit_test_at_object(
        &self,
        page: u32,
        x: f64,
        y: f64,
    ) -> Result<(Hit, bool), EngineError> {
        let hit = self.hit_test_logical(page, x, y)?;
        Ok(match hit {
            Hit::Editable(p) => {
                let (offset, after) = self.text_offset(p);
                (Hit::Editable(p.with_offset(offset)), after)
            }
            other => (other, false),
        })
    }

    /// rhwp's hit test; its offsets count each inline object as a character.
    fn hit_test_logical(&self, page: u32, x: f64, y: f64) -> Result<Hit, EngineError> {
        let s = match self.core.hit_test_native(page, x, y) {
            Ok(s) => s,
            Err(_) => return Ok(Hit::Nothing),
        };
        let v = json(&s)?;
        let section = u(&v, "sectionIndex")?;
        let offset = u(&v, "charOffset")?;
        if v.get("parentParaIndex").is_some() {
            let depth = v
                .get("cellPath")
                .and_then(Value::as_array)
                .map_or(1, Vec::len);
            if depth > 1 {
                return Ok(Hit::ReadOnly(
                    "중첩 표 안의 편집은 아직 지원하지 않습니다".into(),
                ));
            }
            return Ok(Hit::Editable(Pos::Cell {
                section,
                parent_para: u(&v, "parentParaIndex")?,
                control: u(&v, "controlIndex")?,
                cell: u(&v, "cellIndex")?,
                cell_para: u(&v, "cellParaIndex")?,
                offset,
            }));
        }
        if v.get("cellPath").is_some() || v.get("textBox").is_some() {
            return Ok(Hit::ReadOnly(
                "이 개체 안의 편집은 아직 지원하지 않습니다".into(),
            ));
        }
        Ok(Hit::Editable(Pos::Body {
            section,
            para: u(&v, "paragraphIndex")?,
            offset,
        }))
    }

    pub fn caret_rect(&self, pos: Pos) -> Result<CaretRect, EngineError> {
        self.caret_rect_logical(pos.with_offset(self.logical_offset(pos, false)))
    }

    /// The caret right after the inline object at `pos` (see [`Self::hit_test_at_object`]).
    pub fn caret_rect_after_object(&self, pos: Pos) -> Result<CaretRect, EngineError> {
        self.caret_rect_logical(pos.with_offset(self.logical_offset(pos, true)))
    }

    /// Our offsets count text only; rhwp's caret, hit test and paragraph split count each
    /// inline object (수식, 각주·미주, 글자처럼 취급한 그림·표 …) as one character.
    /// `after_object`: the caret is after an object sitting at this text offset.
    fn logical_offset(&self, pos: Pos, after_object: bool) -> usize {
        let Some(p) = self.paragraph(pos) else {
            return pos.offset();
        };
        let at = p.control_text_positions();
        let t = pos.offset();
        let objects = p.controls.iter().enumerate().filter(|(i, c)| {
            let k = at.get(*i).copied().unwrap_or(usize::MAX);
            c.is_logical_inline() && (k < t || after_object && k == t)
        });
        t + objects.count()
    }

    /// The text offset for rhwp's logical `pos`, and whether that place is right after
    /// an inline object.
    fn text_offset(&self, pos: Pos) -> (usize, bool) {
        let Some(p) = self.paragraph(pos) else {
            return (pos.offset(), false);
        };
        let at = p.logical_control_positions();
        let l = pos.offset();
        let inline: Vec<usize> = p
            .controls
            .iter()
            .zip(&at)
            .filter(|(c, _)| c.is_logical_inline())
            .map(|(_, &k)| k)
            .collect();
        let before = inline.iter().filter(|&&k| k < l).count();
        (l - before, inline.iter().any(|&k| k + 1 == l))
    }

    fn caret_rect_logical(&self, pos: Pos) -> Result<CaretRect, EngineError> {
        let s = match pos {
            Pos::Body {
                section,
                para,
                offset,
            } => self.core.get_cursor_rect_native(section, para, offset),
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                cell_para,
                offset,
            } => self.core.get_cursor_rect_in_cell_native(
                section,
                parent_para,
                control,
                cell,
                cell_para,
                offset,
            ),
        }
        .map_err(err)?;
        let v = json(&s)?;
        let cell = v.get("cellBounds").and_then(|b| {
            Some((
                f(b, "x").ok()?,
                f(b, "y").ok()?,
                f(b, "w").ok()?,
                f(b, "h").ok()?,
            ))
        });
        Ok(CaretRect {
            page: u(&v, "pageIndex")? as u32,
            x: f(&v, "x")?,
            y: f(&v, "y")?,
            height: f(&v, "height")?,
            cell,
        })
    }

    fn paragraph(&self, pos: Pos) -> Option<&rhwp::model::paragraph::Paragraph> {
        let doc = self.core.document();
        match pos {
            Pos::Body { section, para, .. } => doc.sections.get(section)?.paragraphs.get(para),
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                cell_para,
                ..
            } => {
                match doc
                    .sections
                    .get(section)?
                    .paragraphs
                    .get(parent_para)?
                    .controls
                    .get(control)?
                {
                    rhwp::model::control::Control::Table(t) => {
                        t.cells.get(cell)?.paragraphs.get(cell_para)
                    }
                    _ => None,
                }
            }
        }
    }

    /// Number of characters (rhwp offsets are `char` indices into `Paragraph::text`) in the paragraph at `pos`.
    pub fn para_len(&self, pos: Pos) -> usize {
        self.paragraph(pos).map_or(0, |p| p.text.chars().count())
    }

    /// Text of a selection inside one paragraph; `None` if it spans paragraphs.
    pub fn text_in(&self, r: Range) -> Option<String> {
        if para_of(r.start) != para_of(r.end) {
            return None;
        }
        let p = self.paragraph(r.start)?;
        Some(
            p.text
                .chars()
                .skip(r.start.offset())
                .take(r.end.offset() - r.start.offset())
                .collect(),
        )
    }

    /// Number of paragraphs in the container of `pos` (body section or cell).
    pub fn para_count(&self, pos: Pos) -> usize {
        let doc = self.core.document();
        match pos {
            Pos::Body { section, .. } => {
                doc.sections.get(section).map_or(0, |s| s.paragraphs.len())
            }
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                ..
            } => doc
                .sections
                .get(section)
                .and_then(|s| s.paragraphs.get(parent_para))
                .and_then(|p| p.controls.get(control))
                .and_then(|c| match c {
                    rhwp::model::control::Control::Table(t) => t.cells.get(cell),
                    _ => None,
                })
                .map_or(0, |c| c.paragraphs.len()),
        }
    }

    /// Whole container of `pos` as a range (Ctrl+A).
    pub fn container_range(&self, pos: Pos) -> Range {
        let last = with_para(pos, self.para_count(pos).saturating_sub(1));
        Range {
            start: with_para(pos, 0).with_offset(0),
            end: last.with_offset(self.para_len(last)),
        }
    }

    /// Insert `text` at `pos`; returns the caret position after the insertion.
    /// The inline object (수식, 각주, 글자처럼 취급한 그림 …) at text offset `pos`, in
    /// body text: the last one there when the caret is after it (Backspace), else the
    /// first (Delete).
    pub fn object_at(&self, pos: Pos, after_object: bool) -> Option<usize> {
        let p = self.paragraph(pos)?;
        let at = p.control_text_positions();
        let mut here = p
            .controls
            .iter()
            .enumerate()
            .filter(|(i, c)| c.is_logical_inline() && at.get(*i) == Some(&pos.offset()))
            .map(|(i, _)| i);
        if after_object {
            here.next_back()
        } else {
            here.next()
        }
    }

    /// Delete the inline object `control` of the body paragraph at `pos` (Backspace after
    /// it, Delete before it).
    pub fn delete_object(&mut self, pos: Pos, control: usize) -> Result<(), EngineError> {
        if let Pos::Cell { .. } = pos {
            let is_equation = self
                .paragraph(pos)
                .and_then(|p| p.controls.get(control))
                .is_some_and(|c| matches!(c, rhwp::model::control::Control::Equation(_)));
            if !is_equation {
                return Err(EngineError(
                    "표 안에서는 수식만 이렇게 지울 수 있습니다".into(),
                ));
            }
        }
        self.mutate(Step::Other, pos, |core| {
            match pos {
                Pos::Body { section, para, .. } => core.delete_control_native(section, para, control),
                Pos::Cell { section, parent_para, control: table, cell, cell_para, .. } => {
                    let path = serde_json::json!([{ "controlIdx": table, "cellIdx": cell, "cellParaIdx": cell_para }]);
                    core.delete_cell_equation_control_by_path_native(section, parent_para, &path.to_string(), control)
                }
            }
            .map(drop)
            .map_err(err)
        })
    }

    /// Insert `text` right after the inline object (수식, 그림, 각주 …) at `pos`;
    /// [`Self::insert_text`] puts it before.
    pub fn insert_text_after_object(&mut self, pos: Pos, text: &str) -> Result<Pos, EngineError> {
        let step = if text.chars().count() == 1 {
            Step::Typing
        } else {
            Step::Other
        };
        self.mutate(step, pos, |core| {
            match pos {
                Pos::Body {
                    section,
                    para,
                    offset,
                } => core.insert_text_after_control_native(section, para, offset, text),
                Pos::Cell {
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    offset,
                } => core.insert_text_in_cell_after_control_native(
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    offset,
                    text,
                ),
            }
            .map_err(err)
        })?;
        Ok(pos.with_offset(pos.offset() + text.chars().count()))
    }

    pub fn insert_text(&mut self, pos: Pos, text: &str) -> Result<Pos, EngineError> {
        let step = if text.chars().count() == 1 {
            Step::Typing
        } else {
            Step::Other
        };
        let r = self.mutate(step, pos, |core| {
            match pos {
                Pos::Body {
                    section,
                    para,
                    offset,
                } => core.insert_text_native(section, para, offset, text),
                Pos::Cell {
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    offset,
                } => core.insert_text_in_cell_native(
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    offset,
                    text,
                ),
            }
            .map_err(err)
        })?;
        let offset = json(&r)?
            .get("charOffset")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(pos.offset() + text.chars().count());
        Ok(pos.with_offset(offset))
    }

    /// Delete `count` characters starting at `pos`.
    pub fn delete_text(&mut self, pos: Pos, count: usize) -> Result<(), EngineError> {
        self.mutate(Step::Deleting, pos, |core| {
            match pos {
                Pos::Body {
                    section,
                    para,
                    offset,
                } => core.delete_text_native(section, para, offset, count),
                Pos::Cell {
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    offset,
                } => core.delete_text_in_cell_native(
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    offset,
                    count,
                ),
            }
            .map_err(err)
        })?;
        Ok(())
    }

    fn delete_range_in(core: &mut DocumentCore, r: Range) -> Result<(), EngineError> {
        let (sp, ep) = (para_of(r.start), para_of(r.end));
        let (so, eo) = (r.start.offset(), r.end.offset());
        match r.start {
            Pos::Body { section, .. } => core.delete_range_native(section, sp, so, ep, eo, None),
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                ..
            } => core.delete_range_native(
                section,
                sp,
                so,
                ep,
                eo,
                Some((parent_para, control, cell)),
            ),
        }
        .map_err(err)?;
        Ok(())
    }

    /// Delete the selection; returns the caret (the range start).
    pub fn delete_range(&mut self, r: Range) -> Result<Pos, EngineError> {
        if !r.is_empty() {
            self.mutate(Step::Other, r.end, |core| Self::delete_range_in(core, r))?;
        }
        Ok(r.start)
    }

    /// Replace the selection with `text` as one undo step.
    pub fn replace_range(&mut self, r: Range, text: &str) -> Result<Pos, EngineError> {
        if r.is_empty() {
            return self.insert_text(r.start, text);
        }
        let p = r.start;
        self.mutate(Step::Other, r.end, |core| {
            Self::delete_range_in(core, r)?;
            match p {
                Pos::Body {
                    section,
                    para,
                    offset,
                } => core.insert_text_native(section, para, offset, text),
                Pos::Cell {
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    offset,
                } => core.insert_text_in_cell_native(
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    offset,
                    text,
                ),
            }
            .map_err(err)
        })?;
        Ok(p.with_offset(p.offset() + text.chars().count()))
    }

    /// Split the paragraph at `pos` (Enter). Returns the caret at the start of the new paragraph.
    pub fn split_paragraph(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        self.split_paragraph_at(pos, false)
    }

    /// Enter right after the inline object at `pos` (see [`Self::insert_text_after_object`]):
    /// the object stays at the end of this paragraph.
    pub fn split_paragraph_after_object(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        self.split_paragraph_at(pos, true)
    }

    fn split_paragraph_at(&mut self, pos: Pos, after_object: bool) -> Result<Pos, EngineError> {
        // rhwp splits at a *logical* offset, which counts each inline object (수식, 그림,
        // 각주 …) as one character; our offsets count text only. Without this, Enter after
        // an object earlier in the paragraph split one character too early per object.
        let logical = {
            // rhwp's split also moves objects that are not drawn inline (Shape, Table …),
            // so count with its own rule here
            let p = self.paragraph(pos);
            p.map_or(pos.offset(), |p| {
                use rhwp::model::control::Control;
                let at = p.control_text_positions();
                let objects = p.controls.iter().enumerate().filter(|(i, c)| {
                    let movable = matches!(
                        c,
                        Control::Shape(_)
                            | Control::Table(_)
                            | Control::Picture(_)
                            | Control::Equation(_)
                            | Control::Footnote(_)
                            | Control::Endnote(_)
                            | Control::AutoNumber(_)
                            | Control::CharOverlap(_)
                    );
                    let k = at.get(*i).copied().unwrap_or(usize::MAX);
                    movable && (k < pos.offset() || after_object && k == pos.offset())
                });
                pos.offset() + objects.count()
            })
        };
        let r = self.mutate(Step::Other, pos, |core| {
            match pos {
                Pos::Body { section, para, .. } => {
                    core.split_paragraph_native(section, para, logical, None)
                }
                Pos::Cell {
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    ..
                } => core.split_paragraph_in_cell_native(
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    logical,
                    None,
                ),
            }
            .map_err(err)
        })?;
        let v = json(&r)?;
        Ok(match pos {
            Pos::Body { section, .. } => Pos::Body {
                section,
                para: u(&v, "paraIdx")?,
                offset: 0,
            },
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
                cell_para: u(&v, "cellParaIndex")?,
                offset: 0,
            },
        })
    }

    /// Join the paragraph at `pos` onto the previous one (Backspace at offset 0).
    /// Returns the caret at the join point, or `None` if `pos` is the first paragraph.
    pub fn merge_with_previous(&mut self, pos: Pos) -> Result<Option<Pos>, EngineError> {
        if para_of(pos) == 0 {
            return Ok(None);
        }
        let r = self.mutate(Step::Other, pos, |core| {
            match pos {
                Pos::Body { section, para, .. } => core.merge_paragraph_native(section, para),
                Pos::Cell {
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    ..
                } => core.merge_paragraph_in_cell_native(
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                ),
            }
            .map_err(err)
        })?;
        let v = json(&r)?;
        let offset = u(&v, "charOffset")?;
        Ok(Some(match pos {
            Pos::Body { section, .. } => Pos::Body {
                section,
                para: u(&v, "paraIdx")?,
                offset,
            },
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
                cell_para: u(&v, "cellParaIndex")?,
                offset,
            },
        }))
    }

    // ------------------------------------------------------------------ formatting

    fn para_lens(&self, r: Range) -> Vec<usize> {
        (para_of(r.start)..=para_of(r.end))
            .map(|i| self.para_len(with_para(r.start, i)))
            .collect()
    }

    /// Apply character properties to every paragraph slice in `r` as one undo step.
    pub fn apply_char_format(&mut self, r: Range, fmt: &CharFormat) -> Result<(), EngineError> {
        self.apply_char_format_ranges(&[r], fmt)
    }

    /// [`Self::apply_char_format`] on several ranges (the cells of a 셀 블록) as one undo step.
    pub fn apply_char_format_ranges(
        &mut self,
        rs: &[Range],
        fmt: &CharFormat,
    ) -> Result<(), EngineError> {
        let rs: Vec<(Range, Vec<usize>)> = rs
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| (*r, self.para_lens(*r)))
            .collect();
        let Some(end) = rs.last().map(|(r, _)| r.end) else {
            return Ok(());
        };
        self.mutate(Step::Other, end, |core| {
            rs.iter()
                .try_for_each(|(r, lens)| apply_char_format_in(core, *r, lens, fmt))
        })
    }

    /// Each cell of the 셀 블록 spanned by `a` and `b` as a range over its whole text,
    /// in the table's cell order — what Hwp formats when cells are selected.
    pub fn cell_block_ranges(&self, a: Pos, b: Pos) -> Result<Vec<Range>, EngineError> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = a
        else {
            return Err(EngineError("표 셀을 고르세요".into()));
        };
        Ok(self
            .cell_block(a, b)?
            .into_iter()
            .map(|(cell, _)| {
                let at = Pos::Cell {
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para: 0,
                    offset: 0,
                };
                self.container_range(at)
            })
            .collect())
    }

    /// Insert `text` with `fmt` applied to it (formatting chosen before typing).
    pub fn insert_text_formatted(
        &mut self,
        pos: Pos,
        text: &str,
        fmt: &CharFormat,
    ) -> Result<Pos, EngineError> {
        let step = if text.chars().count() == 1 {
            Step::Typing
        } else {
            Step::Other
        };
        let end = pos.with_offset(pos.offset() + text.chars().count());
        let r = Range { start: pos, end };
        self.mutate(step, pos, |core| {
            insert_in(core, pos, text)?;
            let lens = [0]; // one paragraph: the slice is bounded by `end`, not by `lens`
            apply_char_format_in(core, r, &lens, fmt)
        })?;
        Ok(end)
    }

    /// Character properties of the character before `pos` (what typing would continue).
    pub fn char_state(&self, pos: Pos) -> Result<CharState, EngineError> {
        let at = pos.offset().saturating_sub(1);
        let s = match pos {
            Pos::Body { section, para, .. } => {
                self.core.get_char_properties_at_native(section, para, at)
            }
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                cell_para,
                ..
            } => self.core.get_cell_char_properties_at_native(
                section,
                parent_para,
                control,
                cell,
                cell_para,
                at,
            ),
        }
        .map_err(err)?;
        let v = json(&s)?;
        let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        Ok(CharState {
            bold: b("bold"),
            italic: b("italic"),
            underline: v
                .get("underlineType")
                .and_then(Value::as_str)
                .map_or(b("underline"), |t| t != "None" && t != "none"),
            strike: b("strikethrough"),
            size_pt: v.get("fontSize").and_then(Value::as_f64).unwrap_or(1000.0) / 100.0,
            color: v
                .get("textColor")
                .and_then(Value::as_str)
                .unwrap_or("#000000")
                .to_owned(),
            font: v
                .get("fontFamily")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        })
    }

    /// Set paragraph alignment for every paragraph touched by `r`.
    pub fn set_align(&mut self, r: Range, align: Align) -> Result<(), EngineError> {
        self.set_align_ranges(&[r], align)
    }

    /// [`Self::set_align`] on several ranges (the cells of a 셀 블록) as one undo step.
    pub fn set_align_ranges(&mut self, rs: &[Range], align: Align) -> Result<(), EngineError> {
        let Some(last) = rs.last() else { return Ok(()) };
        let props = format!("{{\"alignment\":\"{}\"}}", align.key());
        self.mutate(Step::Other, last.end, |core| {
            rs.iter().try_for_each(|r| para_props_in(core, *r, &props))
        })
    }

    /// Apply paragraph properties to every paragraph touched by `r`.
    pub fn set_para_format(&mut self, r: Range, fmt: &ParaFormat) -> Result<(), EngineError> {
        self.set_para_format_ranges(&[r], fmt)
    }

    /// [`Self::set_para_format`] on several ranges (the cells of a 셀 블록) as one undo step.
    pub fn set_para_format_ranges(
        &mut self,
        rs: &[Range],
        fmt: &ParaFormat,
    ) -> Result<(), EngineError> {
        let Some(&r) = rs.last() else { return Ok(()) };
        let mut props = serde_json::Map::new();
        let len = |pt: f64| Value::from((pt * PARA_UNITS_PER_PT).round() as i64);
        match (fmt.line_spacing, fmt.line_spacing_pct) {
            (Some((LineSpacingKind::Percent, v)), _) => {
                props.insert("lineSpacing".into(), (v.round() as i64).into());
                props.insert("lineSpacingType".into(), "Percent".into());
            }
            // stored like the margins, at twice HWPUNIT (rhwp style_resolver, #2070)
            (Some((kind, pt)), _) => {
                props.insert("lineSpacing".into(), len(pt));
                props.insert("lineSpacingType".into(), kind.rhwp().into());
            }
            (None, Some(pct)) => {
                props.insert("lineSpacing".into(), pct.into());
                props.insert("lineSpacingType".into(), "Percent".into());
            }
            (None, None) => {}
        }
        if let Some(v) = fmt.spacing_before_pt {
            props.insert("spacingBefore".into(), len(v));
        }
        if let Some(v) = fmt.spacing_after_pt {
            props.insert("spacingAfter".into(), len(v));
        }
        for (key, v) in [
            ("widowOrphan", fmt.widow_orphan),
            ("keepWithNext", fmt.keep_with_next),
            ("keepLines", fmt.keep_lines),
            ("pageBreakBefore", fmt.page_break_before),
        ] {
            if let Some(v) = v {
                props.insert(key.into(), v.into());
            }
        }
        if let Some(v) = fmt.margin_left_pt {
            props.insert("marginLeft".into(), len(v));
        }
        if let Some(v) = fmt.margin_right_pt {
            props.insert("marginRight".into(), len(v));
        }
        if let Some(v) = fmt.indent_pt {
            props.insert("indent".into(), len(v));
        }
        if props.is_empty() {
            return Ok(());
        }
        let props = Value::Object(props).to_string();
        self.mutate(Step::Other, r.end, |core| {
            rs.iter().try_for_each(|r| para_props_in(core, *r, &props))
        })
    }

    pub fn para_state(&self, pos: Pos) -> Option<ParaState> {
        let s = match pos {
            Pos::Body { section, para, .. } => {
                self.core.get_para_properties_at_native(section, para)
            }
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                cell_para,
                ..
            } => self.core.get_cell_para_properties_at_native(
                section,
                parent_para,
                control,
                cell,
                cell_para,
            ),
        }
        .ok()?;
        let v = json(&s).ok()?;
        let num = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        // rhwp reports px to 0.1 (10 pt -> 13.3 px); round back to the 0.1 pt the UI shows
        let pt = |k: &str| (num(k) * PT_PER_PX * 10.0).round() / 10.0;
        let align = match v.get("alignment").and_then(Value::as_str).unwrap_or("") {
            "center" => Align::Center,
            "right" => Align::Right,
            "justify" => Align::Justify,
            "distribute" => Align::Distribute,
            "split" | "division" => Align::Split,
            _ => Align::Left,
        };
        let kind = match v.get("lineSpacingType").and_then(Value::as_str) {
            Some("Fixed") => LineSpacingKind::Fixed,
            Some("SpaceOnly") => LineSpacingKind::SpaceOnly,
            Some("Minimum") => LineSpacingKind::Minimum,
            _ => LineSpacingKind::Percent,
        };
        let percent = kind == LineSpacingKind::Percent;
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        // spacingBefore/After come as the stored (doubled) HWPUNIT in px; the other
        // line spacings come already halved (rhwp get_para_properties_at_native)
        let doubled = |k: &str| (num(k) * PT_PER_PX * 5.0).round() / 10.0;
        Some(ParaState {
            align,
            line_spacing_pct: percent.then(|| num("lineSpacing")),
            margin_left_pt: pt("marginLeft"),
            margin_right_pt: pt("marginRight"),
            indent_pt: pt("indent"),
            line_spacing: (
                kind,
                if percent {
                    num("lineSpacing")
                } else {
                    pt("lineSpacing")
                },
            ),
            spacing_before_pt: doubled("spacingBefore"),
            spacing_after_pt: doubled("spacingAfter"),
            widow_orphan: flag("widowOrphan"),
            keep_with_next: flag("keepWithNext"),
            keep_lines: flag("keepLines"),
            page_break_before: flag("pageBreakBefore"),
        })
    }

    pub fn align_at(&self, pos: Pos) -> Option<Align> {
        self.para_state(pos).map(|p| p.align)
    }

    // ------------------------------------------------------------------ styles and lists

    /// The document's style list (Hwp's 스타일), in stored order.
    pub fn styles(&self) -> Vec<StyleInfo> {
        let doc = self.core.document();
        doc.doc_info
            .styles
            .iter()
            .enumerate()
            .map(|(id, s)| StyleInfo {
                id,
                name: s.local_name.clone(),
                is_char: s.style_type == 1,
            })
            .collect()
    }

    pub fn style_at(&self, pos: Pos) -> Option<usize> {
        self.paragraph(pos).map(|p| p.style_id as usize)
    }

    /// Apply style `style_id` to every paragraph touched by `r`.
    pub fn apply_style(&mut self, r: Range, style_id: usize) -> Result<(), EngineError> {
        if style_id >= self.core.document().doc_info.styles.len() {
            return Err(EngineError(format!("스타일 {style_id}이(가) 없습니다")));
        }
        self.mutate(Step::Other, r.end, |core| {
            for i in para_of(r.start)..=para_of(r.end) {
                apply_style_at(core, r.start, i, style_id)?;
            }
            Ok(())
        })
    }

    /// The paragraph head at `pos` (개요, 문단 번호 or 글머리표) and its level (0-based).
    pub fn list_at(&self, pos: Pos) -> Option<ListState> {
        let v = json(&para_props(&self.core, pos, para_of(pos)).ok()?).ok()?;
        Some(list_state(&v))
    }

    /// Hwp's 문단 번호 / 글머리표 적용 and 해제 for every paragraph touched by `r`:
    /// `Number` uses the 문단 번호 shape (see `ensure_numbering`), `Bullet` the bullet
    /// shape for `bullet` (added if missing), `None` removes the head. 개요 is a style
    /// in Hwp (`apply_outline`).
    pub fn set_list(&mut self, r: Range, kind: ListKind, bullet: char) -> Result<(), EngineError> {
        if kind == ListKind::Outline {
            return self.apply_outline(r, 0);
        }
        self.mutate(Step::Other, r.end, |core| {
            let props = match kind {
                ListKind::None => "{\"headType\":\"None\"}".to_owned(),
                ListKind::Number => {
                    let id = ensure_numbering(core, section_of(r.start));
                    format!("{{\"headType\":\"Number\",\"numberingId\":{id},\"paraLevel\":0}}")
                }
                _ => {
                    let id = ensure_bullet(core, bullet);
                    format!("{{\"headType\":\"Bullet\",\"numberingId\":{id},\"paraLevel\":0}}")
                }
            };
            for i in para_of(r.start)..=para_of(r.end) {
                apply_para_props(core, r.start, i, &props)?;
            }
            Ok(())
        })
    }

    /// Apply the style 개요 `level + 1` (Hwp's 개요 적용), as Hwp's own template defines it.
    pub fn apply_outline(&mut self, r: Range, level: u8) -> Result<(), EngineError> {
        let id = self.outline_style(level).ok_or_else(|| {
            EngineError(format!(
                "이 문서에는 '개요 {}' 스타일이 없습니다",
                level + 1
            ))
        })?;
        self.apply_style(r, id)
    }

    fn outline_style(&self, level: u8) -> Option<usize> {
        let name = format!("개요 {}", level + 1);
        self.core
            .document()
            .doc_info
            .styles
            .iter()
            .position(|s| s.local_name == name)
    }

    /// Hwp's 한 수준 감소 (`delta` = 1, one level deeper) / 증가 (-1, one level up), as
    /// Hwp 2022 does them: 개요 N moves to the 개요 N±1 style, 개요 1 증가 goes back to
    /// 바탕글 (the first style) and a plain paragraph 감소 becomes 개요 1; a numbered or
    /// bulleted paragraph changes its level within 1–7. Returns whether anything changed.
    pub fn change_level(&mut self, r: Range, delta: i8) -> Result<bool, EngineError> {
        let mut plan = Vec::new();
        for i in para_of(r.start)..=para_of(r.end) {
            let at = with_para(r.start, i);
            let name = self
                .style_at(at)
                .and_then(|s| self.core.document().doc_info.styles.get(s))
                .map(|s| s.local_name.clone());
            let outline = name
                .as_deref()
                .and_then(|n| n.strip_prefix("개요 "))
                .and_then(|n| n.parse::<i8>().ok());
            if let Some(n) = outline {
                let target = n + delta;
                let style = match target {
                    0 => Some(0),
                    1..=7 => self.outline_style((target - 1) as u8),
                    _ => None,
                };
                if let Some(id) = style {
                    plan.push((i, LevelChange::Style(id)));
                }
                continue;
            }
            let Some(list) = self.list_at(at) else {
                continue;
            };
            let level = (list.level as i8 + delta).clamp(0, 6) as u8;
            match list.kind {
                ListKind::Number | ListKind::Bullet if level != list.level => {
                    plan.push((i, LevelChange::Level(level)))
                }
                ListKind::None if delta > 0 => {
                    if let Some(id) = self.outline_style(0) {
                        plan.push((i, LevelChange::Style(id)));
                    }
                }
                _ => {}
            }
        }
        if plan.is_empty() {
            return Ok(false);
        }
        self.mutate(Step::Other, r.end, |core| {
            for (i, change) in &plan {
                match change {
                    LevelChange::Level(level) => {
                        apply_para_props(core, r.start, *i, &format!("{{\"paraLevel\":{level}}}"))?
                    }
                    LevelChange::Style(style) => apply_style_at(core, r.start, *i, *style)?,
                }
            }
            Ok(())
        })?;
        Ok(true)
    }

    // ------------------------------------------------------------------ clipboard

    /// Copy `r` to rhwp's internal (formatted) clipboard; returns its plain text.
    pub fn copy(&mut self, r: Range) -> Result<String, EngineError> {
        let (sp, ep, so, eo) = (
            para_of(r.start),
            para_of(r.end),
            r.start.offset(),
            r.end.offset(),
        );
        let s = match r.start {
            Pos::Body { section, .. } => self.core.copy_selection_native(section, sp, so, ep, eo),
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                ..
            } => self.core.copy_selection_in_cell_native(
                section,
                parent_para,
                control,
                cell,
                sp,
                so,
                ep,
                eo,
            ),
        }
        .map_err(err)?;
        Ok(json(&s)?
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }

    /// Plain text of the internal clipboard, if it holds anything.
    pub fn internal_clipboard_text(&self) -> Option<String> {
        self.core
            .has_internal_clipboard_native()
            .then(|| self.core.get_clipboard_text_native())
    }

    /// Paste the internal clipboard (keeps formatting) at `pos`, replacing `sel`.
    pub fn paste_internal(&mut self, pos: Pos, sel: Option<Range>) -> Result<Pos, EngineError> {
        let caret = sel.map_or(pos, |r| r.end);
        let at = sel.map_or(pos, |r| r.start);
        let r = self.mutate(Step::Other, caret, |core| {
            if let Some(r) = sel.filter(|r| !r.is_empty()) {
                Self::delete_range_in(core, r)?;
            }
            match at {
                Pos::Body {
                    section,
                    para,
                    offset,
                } => core.paste_internal_native(section, para, offset),
                Pos::Cell {
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    offset,
                } => core.paste_internal_in_cell_native(
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    offset,
                ),
            }
            .map_err(err)
        })?;
        let v = json(&r)?;
        if v.get("ok").and_then(Value::as_bool) == Some(false) {
            return Err(EngineError("붙여넣을 내용이 없습니다".into()));
        }
        let offset = u(&v, "charOffset")?;
        Ok(match at {
            Pos::Body { section, .. } => Pos::Body {
                section,
                para: u(&v, "paraIdx")?,
                offset,
            },
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
                cell_para: u(&v, "cellParaIdx")?,
                offset,
            },
        })
    }

    /// 문서 끼워 넣기: the body of another HWP/HWPX file at `pos` (본문만; not in a 표),
    /// with its own 글자·문단 모양 and styles carried over (rhwp merges them into this
    /// document's tables). Its 구역 설정 (paper, margins) is not; one undo step.
    /// Returns the position after the inserted content.
    pub fn insert_document(&mut self, pos: Pos, bytes: &[u8]) -> Result<Pos, EngineError> {
        let Pos::Body {
            section,
            para,
            offset,
        } = pos
        else {
            return Err(EngineError("표 안에는 문서를 끼워 넣을 수 없습니다".into()));
        };
        let foreign = rhwp::parser::parse_document(bytes)
            .map_err(|e| EngineError(format!("끼워 넣을 문서를 읽지 못했습니다: {e}")))?;
        let r = self.mutate(Step::Other, pos, |core| {
            let r = core
                .paste_foreign_document_native(section, para, offset, foreign)
                .map_err(err)?;
            // rhwp's reflow drops the inserted 글머리표·번호 paragraphs' LINE_SEG bit 21,
            // and Hwp 2022 then draws no bullet (f13 check): set it again
            let last = json(&r)
                .ok()
                .and_then(|v| u(&v, "paraIdx").ok())
                .unwrap_or(para);
            for i in para..=last {
                sync_head_tag(core, pos, i);
            }
            Ok(r)
        })?;
        let v = json(&r)?;
        if v.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(EngineError(
                v.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("끼워 넣을 내용이 없습니다")
                    .into(),
            ));
        }
        Ok(Pos::Body {
            section,
            para: u(&v, "paraIdx")?,
            offset: u(&v, "charOffset")?,
        })
    }

    /// Paste plain text (from another application) at `pos`, replacing `sel`.
    /// Line breaks become paragraph breaks; one undo step.
    pub fn paste_text(
        &mut self,
        pos: Pos,
        sel: Option<Range>,
        text: &str,
    ) -> Result<Pos, EngineError> {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let caret = sel.map_or(pos, |r| r.end);
        let start = sel.map_or(pos, |r| r.start);
        self.mutate(Step::Other, caret, |core| {
            if let Some(r) = sel.filter(|r| !r.is_empty()) {
                Self::delete_range_in(core, r)?;
            }
            let mut p = start;
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    p = split_in(core, p)?;
                }
                if !line.is_empty() {
                    insert_in(core, p, line)?;
                    p = p.with_offset(p.offset() + line.chars().count());
                }
            }
            Ok(p)
        })
    }

    // ------------------------------------------------------------------ insert / tables

    /// Page break at `pos` (body text only). Returns the caret at the new page's start.
    pub fn insert_page_break(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        let Pos::Body {
            section,
            para,
            offset,
        } = pos
        else {
            return Err(EngineError("표 안에서는 쪽을 나눌 수 없습니다".into()));
        };
        let r = self.mutate(Step::Other, pos, |core| {
            core.insert_page_break_native(section, para, offset)
                .map_err(err)
        })?;
        Ok(Pos::Body {
            section,
            para: u(&json(&r)?, "paraIdx")?,
            offset: 0,
        })
    }

    /// Paper, orientation and margins of `section` (Hwp's 편집 용지), in millimetres.
    pub fn page_setup(&self, section: usize) -> Option<PageSetup> {
        let v = json(&self.core.get_page_def_native(section).ok()?).ok()?;
        let mm = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0) / HU_PER_MM;
        Some(PageSetup {
            width_mm: mm("width"),
            height_mm: mm("height"),
            landscape: v.get("landscape").and_then(Value::as_bool).unwrap_or(false),
            margin_left_mm: mm("marginLeft"),
            margin_right_mm: mm("marginRight"),
            margin_top_mm: mm("marginTop"),
            margin_bottom_mm: mm("marginBottom"),
            margin_header_mm: mm("marginHeader"),
            margin_footer_mm: mm("marginFooter"),
            margin_gutter_mm: mm("marginGutter"),
        })
    }

    /// Change the paper, orientation and margins of the section holding `caret`.
    pub fn set_page_setup(&mut self, caret: Pos, p: &PageSetup) -> Result<(), EngineError> {
        let hu = |mm: f64| Value::from((mm.max(0.0) * HU_PER_MM).round() as u64);
        let mut m = serde_json::Map::new();
        for (k, v) in [
            ("width", p.width_mm),
            ("height", p.height_mm),
            ("marginLeft", p.margin_left_mm),
            ("marginRight", p.margin_right_mm),
            ("marginTop", p.margin_top_mm),
            ("marginBottom", p.margin_bottom_mm),
            ("marginHeader", p.margin_header_mm),
            ("marginFooter", p.margin_footer_mm),
            ("marginGutter", p.margin_gutter_mm),
        ] {
            m.insert(k.into(), hu(v));
        }
        m.insert("landscape".into(), p.landscape.into());
        let props = Value::Object(m).to_string();
        let section = section_of(caret);
        self.mutate(Step::Other, caret, |core| {
            core.set_page_def_native(section, &props)
                .map(|_| ())
                .map_err(err)
        })
    }

    /// Lay the section holding `caret` out in `count` columns, `gap_mm` apart (Hwp's 단).
    pub fn set_columns(&mut self, caret: Pos, count: u16, gap_mm: f64) -> Result<(), EngineError> {
        if !(1..=255).contains(&count) {
            return Err(EngineError("단 수는 1에서 255 사이여야 합니다".into()));
        }
        let section = section_of(caret);
        let gap = (gap_mm.max(0.0) * HU_PER_MM).round().min(i16::MAX as f64) as i16;
        self.mutate(Step::Other, caret, |core| {
            core.set_column_def_native(section, count, 0, true, gap)
                .map(|_| ())
                .map_err(err)
        })
    }

    /// Start the next column at `pos` (Hwp's 단 나누기, Ctrl+Shift+Enter).
    pub fn insert_column_break(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        let Pos::Body {
            section,
            para,
            offset,
        } = pos
        else {
            return Err(EngineError("표 안에서는 단을 나눌 수 없습니다".into()));
        };
        let r = self.mutate(Step::Other, pos, |core| {
            core.insert_column_break_native(section, para, offset)
                .map_err(err)
        })?;
        Ok(Pos::Body {
            section,
            para: u(&json(&r)?, "paraIdx")?,
            offset: 0,
        })
    }

    /// Hwp's 수식 넣기: put an equation with Hwp script `script` (see [`latex::to_hwp`])
    /// at `pos`, in body text, at 10 pt in black as Hwp's 수식 편집기 does. Returns its
    /// position (see [`Self::insert_picture`]).
    pub fn insert_equation(&mut self, pos: Pos, script: &str) -> Result<Pos, EngineError> {
        self.mutate(Step::Other, pos, |core| {
            match pos {
                Pos::Body {
                    section,
                    para,
                    offset,
                } => core.insert_equation_native(section, para, offset, script, 1000, 0),
                Pos::Cell {
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    offset,
                } => core.insert_equation_in_cell_native(
                    section,
                    parent_para,
                    control,
                    cell,
                    cell_para,
                    offset,
                    script,
                    1000,
                    0,
                ),
            }
            .map_err(err)
        })?;
        Ok(pos)
    }

    /// Hwp's 도형 and 글상자: put a `kind` shape `width_mm` × `height_mm` at `pos` in body
    /// text, treated as a character (글자처럼 취급) so it flows with the text — this
    /// version cannot move floating objects yet. A 글상자 gets `text` (lines become
    /// paragraphs). Returns its position (see [`Self::insert_picture`]).
    pub fn insert_shape(
        &mut self,
        pos: Pos,
        kind: ShapeKind,
        width_mm: f64,
        height_mm: f64,
        text: &str,
    ) -> Result<Pos, EngineError> {
        let Pos::Body {
            section,
            para,
            offset,
        } = pos
        else {
            return Err(EngineError("표 안에는 아직 도형을 넣을 수 없습니다".into()));
        };
        let hu = |mm: f64| (mm.max(0.0) * HU_PER_MM).round() as u32;
        // a line 0 high is not drawn by Hwp 2022 (it opens but shows nothing); 1 HWPUNIT
        // (1/7200 inch) is, in both formats (checked with Hwp)
        let (w, h) = (
            hu(width_mm),
            if kind == ShapeKind::Line {
                1
            } else {
                hu(height_mm)
            },
        );
        if w == 0 && h == 0 {
            return Err(EngineError("도형 크기가 0입니다".into()));
        }
        let name = match kind {
            ShapeKind::Rectangle => "rectangle",
            ShapeKind::Ellipse => "ellipse",
            ShapeKind::Line => "line",
            ShapeKind::TextBox => "textbox",
        };
        self.mutate(Step::Other, pos, |core| {
            let r = core
                .create_shape_control_native(
                    section,
                    para,
                    offset,
                    w,
                    h,
                    0,
                    0,
                    true,
                    "Square",
                    name,
                    false,
                    false,
                    &[],
                )
                .map_err(err)?;
            if kind != ShapeKind::TextBox || text.is_empty() {
                return Ok(());
            }
            let v = json(&r)?;
            let (p, c) = (u(&v, "paraIdx")?, u(&v, "controlIdx")?);
            // rhwp's cell commands take a 글상자 as a one-cell control (as rhwp-studio does)
            let mut prev_len = 0;
            for (i, line) in text.lines().enumerate() {
                if i > 0 {
                    core.split_paragraph_in_cell_native(section, p, c, 0, i - 1, prev_len, None)
                        .map_err(err)?;
                }
                if !line.is_empty() {
                    core.insert_text_in_cell_native(section, p, c, 0, i, 0, line)
                        .map_err(err)?;
                }
                prev_len = line.chars().count();
            }
            Ok(())
        })?;
        Ok(pos)
    }

    /// The text of the 글상자 `control` in the body paragraph at `pos`, one entry per paragraph.
    pub fn text_box_text(&self, pos: Pos, control: usize) -> Option<Vec<String>> {
        use rhwp::model::control::Control;
        let p = self.paragraph(pos)?;
        let Control::Shape(shape) = p.controls.get(control)? else {
            return None;
        };
        let tb = shape.drawing()?.text_box.as_ref()?;
        Some(tb.paragraphs.iter().map(|q| q.text.clone()).collect())
    }

    /// Number of drawing objects (도형, 글상자) in the body paragraph at `pos`.
    pub fn shapes_at(&self, pos: Pos) -> Vec<usize> {
        use rhwp::model::control::Control;
        self.paragraph(pos).map_or(vec![], |p| {
            p.controls
                .iter()
                .enumerate()
                .filter(|(_, c)| matches!(c, Control::Shape(_)))
                .map(|(i, _)| i)
                .collect()
        })
    }

    /// The equation drawn under the point (page px), for double-click editing: the caret
    /// position just before it and its control index.
    /// Where each 수식 on `page` is drawn: (x, y, w, h) in page pixels, body and cells.
    pub fn equation_boxes(&self, page: u32) -> Vec<(f64, f64, f64, f64)> {
        let Some(v) = self
            .core
            .get_page_control_layout_native(page)
            .ok()
            .and_then(|s| json(&s).ok())
        else {
            return Vec::new();
        };
        let f = |c: &Value, k: &str| c.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        v.get("controls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|c| c.get("type").and_then(Value::as_str) == Some("equation"))
            .map(|c| (f(c, "x"), f(c, "y"), f(c, "w"), f(c, "h")))
            .collect()
    }

    pub fn equation_at_point(&self, page: u32, x: f64, y: f64) -> Option<(Pos, usize)> {
        let is_eq = |pos: Pos, c: usize| {
            matches!(
                self.paragraph(pos).and_then(|p| p.controls.get(c)),
                Some(rhwp::model::control::Control::Equation(_))
            )
        };
        let at = |pos: Pos, c: usize| {
            let k = *self.paragraph(pos)?.control_text_positions().get(c)?;
            Some(pos.with_offset(k))
        };
        // body equations: rhwp lists their boxes
        let v = json(&self.core.get_page_control_layout_native(page).ok()?).ok()?;
        let boxed = v.get("controls")?.as_array()?.iter().find(|c| {
            let f = |k: &str| c.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN);
            c.get("type").and_then(Value::as_str) == Some("equation")
                && c.get("noteRef").is_none()
                && c.get("cellIdx").is_none()
                && x >= f("x")
                && x <= f("x") + f("w")
                && y >= f("y")
                && y <= f("y") + f("h")
        });
        if let Some(c) = boxed {
            let pos = Pos::Body {
                section: u(c, "secIdx").ok()?,
                para: u(c, "paraIdx").ok()?,
                offset: 0,
            };
            let control = u(c, "controlIdx").ok()?;
            return is_eq(pos, control)
                .then(|| at(pos, control))
                .flatten()
                .map(|p| (p, control));
        }
        // in a table cell they are not listed: the equation whose two caret stops
        // (before and after it) enclose the point
        let Ok((Hit::Editable(p @ Pos::Cell { .. }), _)) = self.hit_test_at_object(page, x, y)
        else {
            return None;
        };
        let para = self.paragraph(p)?;
        let positions = para.control_text_positions();
        (0..para.controls.len())
            .filter(|&c| is_eq(p, c) && positions.get(c) == Some(&p.offset()))
            .find(|_| {
                let (Ok(r0), Ok(r1)) = (self.caret_rect(p), self.caret_rect_after_object(p)) else {
                    return false;
                };
                r0.page == page
                    && x >= r0.x
                    && x <= r1.x
                    && y >= r0.y - r0.height
                    && y <= r0.y + 2.0 * r0.height
            })
            .map(|c| (p, c))
    }

    /// The Hwp script of equation `control` in the paragraph at `pos`.
    pub fn equation_script(&self, pos: Pos, control: usize) -> Option<String> {
        match self.paragraph(pos)?.controls.get(control)? {
            rhwp::model::control::Control::Equation(eq) => Some(eq.script.clone()),
            _ => None,
        }
    }

    /// The equation just before or at the caret: its control index and Hwp script.
    pub fn equation_near(&self, pos: Pos) -> Option<(usize, String)> {
        let p = self.paragraph(pos)?;
        let at = p.control_text_positions();
        p.controls.iter().enumerate().find_map(|(i, c)| match c {
            rhwp::model::control::Control::Equation(eq)
                if at
                    .get(i)
                    .is_some_and(|&k| k == pos.offset() || k + 1 == pos.offset()) =>
            {
                Some((i, eq.script.clone()))
            }
            _ => None,
        })
    }

    /// Replace the script of the equation `control` in the paragraph at `pos` (수식 고치기).
    pub fn set_equation_script(
        &mut self,
        pos: Pos,
        control: usize,
        script: &str,
    ) -> Result<(), EngineError> {
        let props = serde_json::json!({ "script": script }).to_string();
        self.mutate(Step::Other, pos, |core| {
            match pos {
                Pos::Body { section, para, .. } => {
                    core.set_equation_properties_native(section, para, control, None, None, &props)
                }
                Pos::Cell {
                    section,
                    parent_para,
                    control: table,
                    cell,
                    cell_para,
                    ..
                } => core.set_cell_equation_properties_native(
                    section,
                    parent_para,
                    table,
                    cell,
                    cell_para,
                    control,
                    &props,
                ),
            }
            .map(drop)
            .map_err(err)
        })
    }

    /// How `script` draws, as SVG (for the 수식 dialog's preview).
    pub fn equation_preview_svg(&self, script: &str) -> Result<String, EngineError> {
        self.core
            .render_equation_preview_native(script, 1000, 0)
            .map_err(err)
    }

    /// The 쪽 테두리/배경 of `section`.
    pub fn page_border(&self, section: usize) -> Option<PageBorder> {
        let v = json(&self.core.get_page_border_fill_native(section).ok()?).ok()?;
        let line = |k: &str| {
            let b = v.get(k)?;
            Some(BorderLine {
                kind: b.get("type")?.as_u64()? as u8,
                width: b.get("width")?.as_u64()? as u8,
                color: b.get("color")?.as_str()?.to_owned(),
            })
        };
        let mm = |k: &str| {
            (v.get(k).and_then(Value::as_f64).unwrap_or(0.0) / HU_PER_MM * 100.0).round() / 100.0
        };
        Some(PageBorder {
            borders: [
                line("borderLeft")?,
                line("borderRight")?,
                line("borderTop")?,
                line("borderBottom")?,
            ],
            fill: (v.get("fillType").and_then(Value::as_str) == Some("solid"))
                .then(|| {
                    v.get("fillColor")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .flatten(),
            page_based: v.get("basis").and_then(Value::as_str) == Some("page"),
            spacing_mm: [
                mm("spacingLeft"),
                mm("spacingRight"),
                mm("spacingTop"),
                mm("spacingBottom"),
            ],
            header_inside: v
                .get("headerInside")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            footer_inside: v
                .get("footerInside")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// Hwp's 쪽 테두리/배경 (양쪽, 모든 쪽) for the section holding `caret`. Settings the
    /// dialog does not show (첫 쪽 감추기, 채울 영역) are kept as they are.
    pub fn set_page_border(&mut self, caret: Pos, b: &PageBorder) -> Result<(), EngineError> {
        let section = section_of(caret);
        let current = json(
            &self
                .core
                .get_page_border_fill_native(section)
                .map_err(err)?,
        )?;
        let line = |l: &BorderLine| serde_json::json!({ "type": l.kind, "width": l.width, "color": l.color });
        let hu = |mm: f64| (mm * HU_PER_MM).round() as i64;
        let mut v = serde_json::json!({
            "borderFillId": current.get("borderFillId").cloned().unwrap_or(Value::from(0)),
            "borderLeft": line(&b.borders[0]), "borderRight": line(&b.borders[1]),
            "borderTop": line(&b.borders[2]), "borderBottom": line(&b.borders[3]),
            "basis": if b.page_based { "page" } else { "paper" },
            "spacingLeft": hu(b.spacing_mm[0]), "spacingRight": hu(b.spacing_mm[1]),
            "spacingTop": hu(b.spacing_mm[2]), "spacingBottom": hu(b.spacing_mm[3]),
            "headerInside": b.header_inside, "footerInside": b.footer_inside,
            "hideBorder": current.get("hideBorder").cloned().unwrap_or(Value::Bool(false)),
            "hideFill": current.get("hideFill").cloned().unwrap_or(Value::Bool(false)),
            "fillArea": current.get("fillArea").cloned().unwrap_or(Value::from("paper")),
        });
        match &b.fill {
            Some(c) => {
                v["fillType"] = "solid".into();
                v["fillColor"] = c.as_str().into();
                v["patternType"] = (-1).into();
            }
            None => v["fillType"] = "none".into(),
        }
        let v = v.to_string();
        self.mutate(Step::Other, caret, |core| {
            core.set_page_border_fill_native(section, &v)
                .map(drop)
                .map_err(err)
        })
    }

    /// The 쪽 번호 매기기 setting of `section` (the first one in it), if any.
    pub fn page_number(&self, section: usize) -> Option<PageNumber> {
        self.core
            .document()
            .sections
            .get(section)?
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                rhwp::model::control::Control::PageNumberPos(p) => Some(PageNumber {
                    position: p.position,
                    format: p.format,
                    dash: p.dash_char != '\0',
                }),
                _ => None,
            })
    }

    /// Hwp's 쪽 번호 매기기 for the section holding `caret`: changes the section's existing
    /// setting, or puts one in the caret's paragraph (its body paragraph, from a table).
    /// Written as Hwp writes it: 줄표 넣기 is the dash character `-`, drawn "- 1 -".
    pub fn set_page_number(&mut self, caret: Pos, n: &PageNumber) -> Result<(), EngineError> {
        let section = section_of(caret);
        let here = match caret {
            Pos::Body { para, .. } => para,
            Pos::Cell { parent_para, .. } => parent_para,
        };
        let existing = self.core.document().sections.get(section).and_then(|s| {
            s.paragraphs.iter().position(|p| {
                p.controls
                    .iter()
                    .any(|c| matches!(c, rhwp::model::control::Control::PageNumberPos(_)))
            })
        });
        let pnp = rhwp::model::control::PageNumberPos {
            format: n.format,
            position: n.position,
            dash_char: if n.dash { '-' } else { '\0' },
            ..Default::default()
        };
        self.mutate(Step::Other, caret, |core| {
            core.insert_page_number_pos_native(section, existing.unwrap_or(here), 0, pnp)
                .map(drop)
                .map_err(err)
        })
    }

    /// Hwp's 새 번호로 시작 (쪽 번호): pages count on from `start` at the caret's page.
    /// A 새 번호 already in that paragraph is changed rather than doubled.
    pub fn set_new_page_number(&mut self, caret: Pos, start: u16) -> Result<(), EngineError> {
        use rhwp::model::control::{AutoNumberType, Control};
        let section = section_of(caret);
        let para = match caret {
            Pos::Body { para, .. } => para,
            Pos::Cell { parent_para, .. } => parent_para,
        };
        let existing = self
            .core
            .document()
            .sections
            .get(section)
            .and_then(|s| s.paragraphs.get(para))
            .and_then(|p| {
                p.controls.iter().position(
                    |c| matches!(c, Control::NewNumber(n) if n.number_type == AutoNumberType::Page),
                )
            });
        self.mutate(Step::Other, caret, |core| {
            if let Some(i) = existing {
                core.delete_control_native(section, para, i).map_err(err)?;
            }
            core.insert_new_number_native(section, para, 0, start)
                .map(drop)
                .map_err(err)
        })
    }

    /// The 새 번호로 시작 (쪽) value in the paragraph at `pos`, if it has one.
    pub fn new_page_number_at(&self, pos: Pos) -> Option<u16> {
        let para = match pos {
            Pos::Body { section, para, .. } => self
                .core
                .document()
                .sections
                .get(section)?
                .paragraphs
                .get(para)?,
            Pos::Cell {
                section,
                parent_para,
                ..
            } => self
                .core
                .document()
                .sections
                .get(section)?
                .paragraphs
                .get(parent_para)?,
        };
        para.controls.iter().find_map(|c| match c {
            rhwp::model::control::Control::NewNumber(n)
                if n.number_type == rhwp::model::control::AutoNumberType::Page =>
            {
                Some(n.number)
            }
            _ => None,
        })
    }

    /// Hide parts of the page holding the paragraph at `pos` (Hwp's 현재 쪽만 감추기).
    pub fn hide_on_page(&mut self, pos: Pos, h: PageHide) -> Result<(), EngineError> {
        let Pos::Body { section, para, .. } = pos else {
            return Err(EngineError("표 안에서는 감추기를 넣을 수 없습니다".into()));
        };
        self.mutate(Step::Other, pos, |core| {
            core.set_page_hide_native(
                section,
                para,
                h.header,
                h.footer,
                h.master_page,
                h.border,
                h.fill,
                h.page_number,
            )
            .map(|_| ())
            .map_err(err)
        })
    }

    /// Insert an image file's bytes at `pos` as a picture `width_mm` wide, keeping its
    /// aspect ratio; `px` is the image's size in pixels. Returns its position: an inline
    /// object takes no text offset, so text typed after it goes through
    /// [`Self::insert_text_after_object`].
    pub fn insert_picture(
        &mut self,
        pos: Pos,
        bytes: &[u8],
        extension: &str,
        px: (u32, u32),
        width_mm: f64,
    ) -> Result<Pos, EngineError> {
        if px.0 == 0 || px.1 == 0 {
            return Err(EngineError("그림 크기를 알 수 없습니다".into()));
        }
        let w = (width_mm.max(1.0) * HU_PER_MM).round() as u32;
        let h = ((w as f64) * px.1 as f64 / px.0 as f64).round().max(1.0) as u32;
        let (section, para, offset, path) = match pos {
            Pos::Body {
                section,
                para,
                offset,
            } => (section, para, offset, vec![]),
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                cell_para,
                offset,
            } => (
                section,
                parent_para,
                offset,
                vec![(control, cell, cell_para)],
            ),
        };
        let ext = extension.to_ascii_lowercase();
        self.mutate(Step::Other, pos, |core| {
            core.insert_picture_native(
                section, para, offset, &path, bytes, w, h, px.0, px.1, &ext, "", None, None,
            )
            .map(|_| ())
            .map_err(err)
        })?;
        Ok(pos)
    }

    /// Insert a footnote (or an endnote) at `pos` holding `text`; returns its number's
    /// position in the body (see [`Self::insert_picture`]).
    pub fn insert_note(&mut self, pos: Pos, endnote: bool, text: &str) -> Result<Pos, EngineError> {
        let Pos::Body {
            section,
            para,
            offset,
        } = pos
        else {
            return Err(EngineError(
                "표 안에는 아직 각주·미주를 넣을 수 없습니다".into(),
            ));
        };
        self.mutate(Step::Other, pos, |core| {
            let r = if endnote {
                core.insert_endnote_native(section, para, offset)
            } else {
                core.insert_footnote_native(section, para, offset)
            };
            let v = json(&r.map_err(err)?)?;
            let (p, c) = (u(&v, "paraIdx")?, u(&v, "controlIdx")?);
            if !text.is_empty() {
                // the note paragraph starts with two placeholder spaces around its number;
                // text goes after them (rhwp, like Hwp, starts input at offset 2)
                core.insert_text_in_footnote_native(section, p, c, 0, 2, text)
                    .map_err(err)?;
            }
            Ok(())
        })?;
        Ok(pos.with_offset(offset))
    }

    /// Text of the section's header (or footer) on all pages, if it has one.
    pub fn header_footer_text(&self, caret: Pos, header: bool) -> Option<String> {
        let section = section_of(caret);
        let v = json(
            &self
                .core
                .get_header_footer_native(section, header, 0)
                .ok()?,
        )
        .ok()?;
        if v.get("exists").and_then(Value::as_bool) == Some(false) {
            return None;
        }
        let info = json(
            &self
                .core
                .get_header_footer_para_info_native(section, header, 0, 0)
                .ok()?,
        )
        .ok()?;
        Some(
            info.get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        )
    }

    /// Make the section's header (or footer) for all pages read `text` (one line),
    /// creating it if needed.
    pub fn set_header_footer_text(
        &mut self,
        caret: Pos,
        header: bool,
        text: &str,
    ) -> Result<(), EngineError> {
        let section = section_of(caret);
        let exists = self.header_footer_text(caret, header).is_some();
        self.mutate(Step::Other, caret, |core| {
            if !exists {
                core.create_header_footer_native(section, header, 0)
                    .map_err(err)?;
            }
            let info = json(
                &core
                    .get_header_footer_para_info_native(section, header, 0, 0)
                    .map_err(err)?,
            )?;
            let n = info.get("charCount").and_then(Value::as_u64).unwrap_or(0) as usize;
            if n > 0 {
                core.delete_text_in_header_footer_native(section, header, 0, 0, 0, n)
                    .map_err(err)?;
            }
            if !text.is_empty() {
                core.insert_text_in_header_footer_native(section, header, 0, 0, 0, text)
                    .map_err(err)?;
            }
            Ok(())
        })
    }

    /// Pictures in the body and in (non-nested) table cells.
    /// Whether the first stored line of the paragraph at `pos` has the 문단 머리 flag
    /// (LINE_SEG bit 21); `None` when the paragraph has no stored lines.
    pub fn first_line_has_head_tag(&self, pos: Pos) -> Option<bool> {
        self.paragraph(pos)?
            .line_segs
            .first()
            .map(|l| l.has_paragraph_head())
    }

    pub fn picture_count(&self) -> usize {
        use rhwp::model::control::Control;
        let pics = |ps: &[rhwp::model::paragraph::Paragraph]| -> usize {
            ps.iter()
                .map(|p| {
                    p.controls
                        .iter()
                        .filter(|c| matches!(c, Control::Picture(_)))
                        .count()
                })
                .sum()
        };
        self.core
            .document()
            .sections
            .iter()
            .flat_map(|s| s.paragraphs.iter())
            .map(|p| {
                pics(std::slice::from_ref(p))
                    + p.controls
                        .iter()
                        .map(|c| match c {
                            Control::Table(t) => {
                                t.cells.iter().map(|cell| pics(&cell.paragraphs)).sum()
                            }
                            _ => 0,
                        })
                        .sum::<usize>()
            })
            .sum()
    }

    /// Add a bookmark named `name` at `pos` (body text).
    pub fn add_bookmark(&mut self, pos: Pos, name: &str) -> Result<(), EngineError> {
        let Pos::Body {
            section,
            para,
            offset,
        } = pos
        else {
            return Err(EngineError(
                "표 안에는 아직 책갈피를 넣을 수 없습니다".into(),
            ));
        };
        self.mutate(Step::Other, pos, |core| {
            let r = core
                .add_bookmark_native(section, para, offset, name)
                .map_err(err)?;
            match json(&r)?.get("ok").and_then(Value::as_bool) {
                Some(false) => Err(EngineError(
                    json(&r)?
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("책갈피를 넣지 못했습니다")
                        .to_owned(),
                )),
                _ => Ok(()),
            }
        })
    }

    /// Names of the document's bookmarks.
    pub fn bookmarks(&self) -> Vec<String> {
        let Ok(v) = self
            .core
            .get_bookmarks_native()
            .map_err(err)
            .and_then(|s| json(&s))
        else {
            return Vec::new();
        };
        let list = v
            .as_array()
            .cloned()
            .or_else(|| v.get("bookmarks").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        list.iter()
            .filter_map(|b| b.get("name").and_then(Value::as_str).map(str::to_owned))
            .collect()
    }

    /// Make the selected text (inside one paragraph) a link to `uri` (http/https).
    pub fn insert_hyperlink(&mut self, r: Range, uri: &str) -> Result<(), EngineError> {
        let target = match r.start {
            Pos::Body { section, para, .. } => {
                rhwp::document_core::hyperlink::HyperlinkTarget::body(section, para)
            }
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                cell_para,
                ..
            } => rhwp::document_core::hyperlink::HyperlinkTarget {
                section,
                para: parent_para,
                cell_path: vec![(control, cell, cell_para)],
            },
        };
        if para_of(r.start) != para_of(r.end) || r.start.offset() == r.end.offset() {
            return Err(EngineError("한 문단 안의 글자를 골라 주세요".into()));
        }
        let (a, b) = (r.start.offset(), r.end.offset());
        self.mutate(Step::Other, r.end, |core| {
            core.insert_hyperlink_native(&target, a, b, uri)
                .map(|_| ())
                .map_err(err)
        })
    }

    /// Insert a `rows` x `cols` table at `pos` (body text only); returns the caret in its first cell.
    pub fn create_table(&mut self, pos: Pos, rows: u16, cols: u16) -> Result<Pos, EngineError> {
        let Pos::Body {
            section,
            para,
            offset,
        } = pos
        else {
            return Err(EngineError("표 안에는 표를 만들 수 없습니다".into()));
        };
        if rows == 0 || cols == 0 {
            return Err(EngineError("줄과 칸은 1 이상이어야 합니다".into()));
        }
        let r = self.mutate(Step::Other, pos, |core| {
            core.create_table_native(section, para, offset, rows, cols)
                .map_err(err)
        })?;
        let v = json(&r)?;
        let t = Pos::Cell {
            section,
            parent_para: u(&v, "paraIdx")?,
            control: u(&v, "controlIdx")?,
            cell: 0,
            cell_para: 0,
            offset: 0,
        };
        self.sync_table_flow(t);
        Ok(t)
    }

    fn table(&self, pos: Pos) -> Option<&rhwp::model::table::Table> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = pos
        else {
            return None;
        };
        match self
            .core
            .document()
            .sections
            .get(section)?
            .paragraphs
            .get(parent_para)?
            .controls
            .get(control)?
        {
            rhwp::model::control::Control::Table(t) => Some(t),
            _ => None,
        }
    }

    pub fn cell_info(&self, pos: Pos) -> Option<CellInfo> {
        let Pos::Cell { cell, .. } = pos else {
            return None;
        };
        let t = self.table(pos)?;
        let c = t.cells.get(cell)?;
        Some(CellInfo {
            row: c.row,
            col: c.col,
            row_span: c.row_span,
            col_span: c.col_span,
            rows: t.row_count,
            cols: t.col_count,
        })
    }

    /// Caret at the start of the cell covering (row, col) in the table of `pos`.
    pub fn cell_at(&self, pos: Pos, row: u16, col: u16) -> Option<Pos> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = pos
        else {
            return None;
        };
        let t = self.table(pos)?;
        let i = t.cells.iter().position(|c| {
            (c.row..c.row + c.row_span.max(1)).contains(&row)
                && (c.col..c.col + c.col_span.max(1)).contains(&col)
        })?;
        Some(Pos::Cell {
            section,
            parent_para,
            control,
            cell: i,
            cell_para: 0,
            offset: 0,
        })
    }

    /// Run a structural table command, then put the caret in the cell at (row, col).
    fn table_op(
        &mut self,
        pos: Pos,
        f: impl FnOnce(&mut DocumentCore, usize, usize, usize, CellInfo) -> Result<String, HwpError>,
        caret: impl FnOnce(CellInfo, &Document) -> (u16, u16),
    ) -> Result<Pos, EngineError> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = pos
        else {
            return Err(EngineError("캐럿을 표 안에 두세요".into()));
        };
        let info = self
            .cell_info(pos)
            .ok_or_else(|| EngineError("표 셀을 찾을 수 없습니다".into()))?;
        self.mutate(Step::Other, pos, |core| {
            f(core, section, parent_para, control, info).map_err(err)
        })?;
        let (r, c) = caret(info, self);
        Ok(self
            .cell_at(pos, r, c)
            .or_else(|| self.cell_at(pos, 0, 0))
            .unwrap_or(pos))
    }

    pub fn insert_row(&mut self, pos: Pos, below: bool) -> Result<Pos, EngineError> {
        self.table_op(
            pos,
            |core, s, p, c, i| {
                core.insert_table_row_native(
                    s,
                    p,
                    c,
                    if below {
                        i.row + i.row_span.max(1) - 1
                    } else {
                        i.row
                    },
                    below,
                )
            },
            |i, _| (if below { i.row } else { i.row + 1 }, i.col),
        )
    }

    pub fn insert_col(&mut self, pos: Pos, right: bool) -> Result<Pos, EngineError> {
        self.table_op(
            pos,
            |core, s, p, c, i| {
                core.insert_table_column_native(
                    s,
                    p,
                    c,
                    if right {
                        i.col + i.col_span.max(1) - 1
                    } else {
                        i.col
                    },
                    right,
                )
            },
            |i, _| (i.row, if right { i.col } else { i.col + 1 }),
        )
    }

    pub fn delete_row(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        self.counting_removals(|d| {
            if d.cell_info(pos).is_some_and(|i| i.rows <= 1) {
                return d.remove_table(pos);
            }
            d.table_op(
                pos,
                |core, s, p, c, i| core.delete_table_row_native(s, p, c, i.row),
                |i, d| {
                    let rows = d.cell_info(pos).map_or(1, |n| n.rows);
                    (i.row.min(rows.saturating_sub(1)), i.col)
                },
            )
        })
    }

    pub fn delete_col(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        self.counting_removals(|d| {
            if d.cell_info(pos).is_some_and(|i| i.cols <= 1) {
                return d.remove_table(pos);
            }
            d.table_op(
                pos,
                |core, s, p, c, i| core.delete_table_column_native(s, p, c, i.col),
                |i, d| {
                    let cols = d.cell_info(pos).map_or(1, |n| n.cols);
                    (i.row, i.col.min(cols.saturating_sub(1)))
                },
            )
        })
    }

    /// Merge the rectangle of cells spanned by `a` and `b` (cells of one table).
    pub fn merge_cells(&mut self, a: Pos, b: Pos) -> Result<Pos, EngineError> {
        if !same_table(a, b) {
            return Err(EngineError("합칠 셀을 같은 표 안에서 고르세요".into()));
        }
        let (ia, ib) = (self.cell_info(a), self.cell_info(b));
        let (Some(ia), Some(ib)) = (ia, ib) else {
            return Err(EngineError("표 셀을 찾을 수 없습니다".into()));
        };
        let r0 = ia.row.min(ib.row);
        let c0 = ia.col.min(ib.col);
        let r1 = (ia.row + ia.row_span.max(1)).max(ib.row + ib.row_span.max(1)) - 1;
        let c1 = (ia.col + ia.col_span.max(1)).max(ib.col + ib.col_span.max(1)) - 1;
        if (r0, c0) == (r1, c1) {
            return Err(EngineError("두 칸 이상을 고르세요".into()));
        }
        self.table_op(
            a,
            |core, s, p, c, _| core.merge_table_cells_native(s, p, c, r0, c0, r1, c1),
            |_, _| (r0, c0),
        )
    }

    // ------------------------------------------------------------------ cell borders and shading

    /// The borders (left, right, top, bottom) and 셀 음영 of the cell at `pos`, as drawn
    /// (a border set on a block of cells counts).
    pub fn cell_look(&self, pos: Pos) -> Option<CellLook> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            ..
        } = pos
        else {
            return None;
        };
        let v = json(
            &self
                .core
                .get_cell_properties_by_cell_path_native(
                    section,
                    parent_para,
                    &[(control, cell, 0)],
                    cell,
                )
                .ok()?,
        )
        .ok()?;
        let line = |k: &str| {
            let b = v.get(k)?;
            Some(BorderLine {
                kind: b.get("type")?.as_u64()? as u8,
                width: b.get("width")?.as_u64()? as u8,
                color: b.get("color")?.as_str()?.to_owned(),
            })
        };
        let fill = (v.get("fillType").and_then(Value::as_str) == Some("solid"))
            .then(|| {
                v.get("fillColor")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .flatten();
        Some(CellLook {
            borders: [
                line("borderLeft")?,
                line("borderRight")?,
                line("borderTop")?,
                line("borderBottom")?,
            ],
            fill,
        })
    }

    /// Hwp's 셀 음영: fill every cell of the block spanned by `a` and `b` with `color`
    /// ("#rrggbb"), or remove the fill (`None`).
    pub fn set_cell_fill(
        &mut self,
        a: Pos,
        b: Pos,
        color: Option<&str>,
    ) -> Result<(), EngineError> {
        let cells = self.cell_block(a, b)?;
        let color = color.map(str::to_owned);
        self.edit_cells(a, cells, move |v, _| {
            match &color {
                Some(c) => {
                    v.insert("fillType".into(), "solid".into());
                    v.insert("fillColor".into(), c.as_str().into());
                    // a plain colour: no hatch, as Hwp writes it (no hatchStyle)
                    v.insert("patternType".into(), (-1).into());
                }
                None => {
                    v.insert("fillType".into(), "none".into());
                }
            }
        })
    }

    /// Hwp's 테두리: draw `line` on the chosen edges of the block spanned by `a` and `b`
    /// (`kind` 0 removes the line).
    pub fn set_cell_borders(
        &mut self,
        a: Pos,
        b: Pos,
        sides: BorderSides,
        line: &BorderLine,
    ) -> Result<(), EngineError> {
        let cells = self.cell_block(a, b)?;
        let border =
            serde_json::json!({ "type": line.kind, "width": line.width, "color": line.color });
        self.edit_cells(a, cells, move |v, edges| {
            for (key, outer) in ["borderLeft", "borderRight", "borderTop", "borderBottom"]
                .into_iter()
                .zip(edges)
            {
                let on = match sides {
                    BorderSides::All => true,
                    BorderSides::Outer => outer,
                    BorderSides::Inner => !outer,
                    BorderSides::Left => key == "borderLeft" && outer,
                    BorderSides::Right => key == "borderRight" && outer,
                    BorderSides::Top => key == "borderTop" && outer,
                    BorderSides::Bottom => key == "borderBottom" && outer,
                    BorderSides::InnerHorizontal => {
                        !outer && matches!(key, "borderTop" | "borderBottom")
                    }
                    BorderSides::InnerVertical => {
                        !outer && matches!(key, "borderLeft" | "borderRight")
                    }
                };
                if on {
                    v.insert(key.into(), border.clone());
                }
            }
        })
    }

    /// The cells of the rectangle spanned by corner cells `a` and `b`, each with which of
    /// its edges (left, right, top, bottom) lie on the rectangle's outline.
    fn cell_block(&self, a: Pos, b: Pos) -> Result<Vec<(usize, [bool; 4])>, EngineError> {
        if !same_table(a, b) {
            return Err(EngineError("같은 표 안의 셀을 고르세요".into()));
        }
        let (Some(ia), Some(ib), Some(t)) = (self.cell_info(a), self.cell_info(b), self.table(a))
        else {
            return Err(EngineError("표 셀을 찾을 수 없습니다".into()));
        };
        let r0 = ia.row.min(ib.row);
        let c0 = ia.col.min(ib.col);
        let r1 = (ia.row + ia.row_span.max(1)).max(ib.row + ib.row_span.max(1)) - 1;
        let c1 = (ia.col + ia.col_span.max(1)).max(ib.col + ib.col_span.max(1)) - 1;
        Ok(t.cells
            .iter()
            .enumerate()
            .filter(|(_, c)| (r0..=r1).contains(&c.row) && (c0..=c1).contains(&c.col))
            .map(|(i, c)| {
                let (last_row, last_col) =
                    (c.row + c.row_span.max(1) - 1, c.col + c.col_span.max(1) - 1);
                (
                    i,
                    [c.col == c0, last_col == c1, c.row == r0, last_row == r1],
                )
            })
            .collect())
    }

    /// Change the border/fill properties of `cells` (one undo step). `f` edits each
    /// cell's current properties as rhwp reports them; rhwp then stores a matching
    /// border-fill entry (reusing an equal one) and keeps neighbours' shared edges in step.
    fn table_model(&self, at: Pos) -> Option<&rhwp::model::table::Table> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = at
        else {
            return None;
        };
        match self
            .core
            .document()
            .sections
            .get(section)?
            .paragraphs
            .get(parent_para)?
            .controls
            .get(control)?
        {
            rhwp::model::control::Control::Table(t) => Some(t),
            _ => None,
        }
    }

    /// 셀 속성 of the cell at `at`.
    pub fn cell_props(&self, at: Pos) -> Option<CellProps> {
        let Pos::Cell { cell, .. } = at else {
            return None;
        };
        let c = self.table_model(at)?.cells.get(cell)?;
        let mm = |v: i16| v as f64 / HU_PER_MM;
        Some(CellProps {
            vertical_align: match c.vertical_align {
                rhwp::model::table::VerticalAlign::Top => 0,
                rhwp::model::table::VerticalAlign::Center => 1,
                rhwp::model::table::VerticalAlign::Bottom => 2,
            },
            own_margins: c.apply_inner_margin,
            margins_mm: [
                mm(c.padding.left),
                mm(c.padding.right),
                mm(c.padding.top),
                mm(c.padding.bottom),
            ],
            header: c.is_header,
        })
    }

    /// Set 셀 속성 on every cell of the block spanned by `a` and `b`; `None` parts stay.
    pub fn set_cell_props(
        &mut self,
        a: Pos,
        b: Pos,
        vertical_align: Option<u8>,
        margins_mm: Option<Option<[f64; 4]>>,
        header: Option<bool>,
    ) -> Result<(), EngineError> {
        let cells = self.cell_block(a, b)?;
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = a
        else {
            return Err(EngineError("표 셀을 고르세요".into()));
        };
        let mut v = serde_json::Map::new();
        if let Some(va) = vertical_align {
            v.insert("verticalAlign".into(), va.min(2).into());
        }
        match margins_mm {
            Some(Some(m)) => {
                v.insert("applyInnerMargin".into(), true.into());
                for (k, mm) in ["paddingLeft", "paddingRight", "paddingTop", "paddingBottom"]
                    .into_iter()
                    .zip(m)
                {
                    v.insert(k.into(), ((mm * HU_PER_MM).round() as i64).into());
                }
            }
            Some(None) => {
                v.insert("applyInnerMargin".into(), false.into());
            }
            None => {}
        }
        if let Some(h) = header {
            v.insert("isHeader".into(), h.into());
        }
        if v.is_empty() {
            return Ok(());
        }
        let props = Value::Object(v).to_string();
        self.mutate(Step::Other, a, |core| {
            for (cell, _) in cells {
                core.set_cell_properties_native(section, parent_para, control, cell, &props)
                    .map_err(err)?;
            }
            Ok(())
        })?;
        Ok(())
    }

    /// 표 속성 of the table holding `at`.
    pub fn table_props(&self, at: Pos) -> Option<TableProps> {
        let t = self.table_model(at)?;
        let mm = |v: i16| v as f64 / HU_PER_MM;
        Some(TableProps {
            page_break: match t.page_break {
                rhwp::model::table::TablePageBreak::None => 0,
                rhwp::model::table::TablePageBreak::CellBreak => 1,
                rhwp::model::table::TablePageBreak::RowBreak => 2,
            },
            repeat_header: t.repeat_header,
            margins_mm: [
                mm(t.padding.left),
                mm(t.padding.right),
                mm(t.padding.top),
                mm(t.padding.bottom),
            ],
        })
    }

    pub fn set_table_props(&mut self, at: Pos, p: &TableProps) -> Result<(), EngineError> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = at
        else {
            return Err(EngineError("표 안에 캐럿을 두세요".into()));
        };
        let hu = |mm: f64| (mm * HU_PER_MM).round() as i64;
        let props = serde_json::json!({
            "pageBreak": p.page_break.min(2),
            "repeatHeader": p.repeat_header,
            "paddingLeft": hu(p.margins_mm[0]),
            "paddingRight": hu(p.margins_mm[1]),
            "paddingTop": hu(p.margins_mm[2]),
            "paddingBottom": hu(p.margins_mm[3]),
        })
        .to_string();
        self.mutate(Step::Other, at, |core| {
            core.set_table_properties_native(section, parent_para, control, &props)
                .map_err(err)
        })?;
        Ok(())
    }

    fn edit_cells(
        &mut self,
        at: Pos,
        cells: Vec<(usize, [bool; 4])>,
        f: impl Fn(&mut serde_json::Map<String, Value>, [bool; 4]),
    ) -> Result<(), EngineError> {
        const KEYS: [&str; 14] = [
            "borderFillId",
            "borderLeft",
            "borderRight",
            "borderTop",
            "borderBottom",
            "fillType",
            "fillColor",
            "patternColor",
            "patternType",
            "diagonalLine",
            "diagonalSlash",
            "diagonalBackSlash",
            "diagonalWidth",
            "diagonalColor",
        ];
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = at
        else {
            return Err(EngineError("표 셀을 고르세요".into()));
        };
        self.mutate(Step::Other, at, |core| {
            for (cell, edges) in cells {
                let props = core
                    .get_cell_properties_by_cell_path_native(
                        section,
                        parent_para,
                        &[(control, cell, 0)],
                        cell,
                    )
                    .map_err(err)?;
                let Value::Object(all) = json(&props)? else {
                    return Err(EngineError(format!("셀 속성을 읽을 수 없습니다: {props}")));
                };
                let mut v: serde_json::Map<String, Value> = all
                    .into_iter()
                    .filter(|(k, _)| KEYS.contains(&k.as_str()) || k == "centerLine")
                    .collect();
                f(&mut v, edges);
                core.set_cell_properties_native(
                    section,
                    parent_para,
                    control,
                    cell,
                    &Value::Object(v).to_string(),
                )
                .map_err(err)?;
            }
            Ok(())
        })
    }

    /// Undo a merge: split the cell at `pos` back into single cells.
    pub fn split_cell(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        if self
            .cell_info(pos)
            .is_some_and(|i| i.row_span <= 1 && i.col_span <= 1)
        {
            return Err(EngineError("합쳐진 셀이 아닙니다".into()));
        }
        self.table_op(
            pos,
            |core, s, p, c, i| core.split_table_cell_native(s, p, c, i.row, i.col),
            |i, _| (i.row, i.col),
        )
    }

    /// The next (or previous) cell in reading order, by each cell's top-left grid position.
    pub fn next_cell(&self, pos: Pos, forward: bool) -> Option<Pos> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            ..
        } = pos
        else {
            return None;
        };
        let t = self.table(pos)?;
        let mut order: Vec<usize> = (0..t.cells.len()).collect();
        order.sort_by_key(|&i| (t.cells[i].row, t.cells[i].col));
        let k = order.iter().position(|&i| i == cell)?;
        let next = if forward {
            order.get(k + 1)
        } else {
            k.checked_sub(1).and_then(|j| order.get(j))
        };
        next.map(|&i| Pos::Cell {
            section,
            parent_para,
            control,
            cell: i,
            cell_para: 0,
            offset: 0,
        })
    }

    /// Tab in a table, as in Hwp: go to the next cell; from the last cell, add a
    /// row below and go to its first cell.
    pub fn tab_forward(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        if let Some(n) = self.next_cell(pos, true) {
            return Ok(n);
        }
        let rows = self
            .cell_info(pos)
            .map(|i| i.rows)
            .ok_or_else(|| EngineError("캐럿을 표 안에 두세요".into()))?;
        self.insert_row(pos, true)?;
        Ok(self.cell_at(pos, rows, 0).unwrap_or(pos))
    }

    /// Boxes of the cells of the table at `pos` that lie on `page`.
    pub fn cell_boxes(&self, pos: Pos, page: u32) -> Vec<CellBox> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = pos
        else {
            return Vec::new();
        };
        let Ok(s) =
            self.core
                .get_table_cell_bboxes_from_page(section, parent_para, control, page as usize)
        else {
            return Vec::new();
        };
        let Ok(Value::Array(a)) = serde_json::from_str::<Value>(&s) else {
            return Vec::new();
        };
        let n = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        a.iter()
            .map(|v| CellBox {
                cell: n(v, "cellIdx") as usize,
                row: n(v, "row") as u16,
                col: n(v, "col") as u16,
                row_span: n(v, "rowSpan") as u16,
                col_span: n(v, "colSpan") as u16,
                page: n(v, "pageIndex") as u32,
                x: n(v, "x"),
                y: n(v, "y"),
                w: n(v, "w"),
                h: n(v, "h"),
            })
            .filter(|b| b.page == page)
            .collect()
    }

    /// Column widths of the table at `pos` in HWPUNIT: the stored width of a
    /// single-column cell where there is one, otherwise (merged-only columns, or the
    /// zero "automatic" widths of a freshly created table) the rendered width.
    /// Untouched columns therefore keep their exact stored values.
    fn column_widths(&self, pos: Pos) -> Option<Vec<u32>> {
        let t = self.table(pos)?;
        let n = t.col_count as usize;
        let mut w: Vec<Option<u32>> = vec![None; n];
        for c in &t.cells {
            if c.col_span == 1 && (c.col as usize) < n && c.width > 0 {
                w[c.col as usize] = Some(w[c.col as usize].map_or(c.width, |x| x.max(c.width)));
            }
        }
        if w.iter().any(Option::is_none) {
            let page = self.caret_rect(pos).ok()?.page;
            let boxes = self.cell_boxes(pos, page);
            for (i, slot) in w.iter_mut().enumerate() {
                if slot.is_none() {
                    let b = boxes
                        .iter()
                        .find(|b| b.col as usize == i && b.col_span == 1)?;
                    *slot = Some((b.w * HWPUNIT_PER_PX).round() as u32);
                }
            }
        }
        w.into_iter().collect()
    }

    /// Resize the column (`vertical == false`) or row of the cell at `pos` by `steps`
    /// Hwp steps (negative shrinks), as one undo step.
    pub fn resize_table(
        &mut self,
        pos: Pos,
        vertical: bool,
        steps: i32,
        mode: Resize,
    ) -> Result<Pos, EngineError> {
        if steps == 0 {
            return Ok(pos);
        }
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = pos
        else {
            return Err(EngineError("캐럿을 표 안에 두세요".into()));
        };
        let info = self
            .cell_info(pos)
            .ok_or_else(|| EngineError("표 셀을 찾을 수 없습니다".into()))?;
        let delta = steps as i64 * RESIZE_STEP_HWPUNIT as i64;
        if !vertical {
            // columns: set all widths at once, which also handles merged cells
            let mut w: Vec<i64> = self
                .column_widths(pos)
                .ok_or_else(|| EngineError("열 너비를 알 수 없습니다".into()))?
                .into_iter()
                .map(i64::from)
                .collect();
            let c = (info.col + info.col_span.max(1) - 1) as usize;
            const MIN: i64 = 2 * RESIZE_STEP_HWPUNIT as i64;
            w[c] += delta;
            if mode == Resize::Border && c + 1 < w.len() {
                w[c + 1] -= delta;
            }
            if w.iter().any(|&x| x < MIN) {
                return Err(EngineError("칸이 너무 좁아집니다".into()));
            }
            let widths: Vec<u32> = w.into_iter().map(|x| x as u32).collect();
            self.mutate(Step::Other, pos, |core| {
                core.set_table_column_widths_native(section, parent_para, control, widths)
                    .map_err(err)
            })?;
        } else {
            let row = info.row + info.row_span.max(1) - 1;
            let col = info.col;
            self.mutate(Step::Other, pos, |core| {
                for _ in 0..steps.unsigned_abs() {
                    let r = core
                        .resize_table_native(
                            section,
                            parent_para,
                            control,
                            row,
                            col,
                            true,
                            steps > 0,
                            mode == Resize::Border,
                        )
                        .map_err(err)?;
                    let v = json(&r)?;
                    if v.get("ok").and_then(Value::as_bool) == Some(false) {
                        let why = v
                            .get("reason")
                            .and_then(Value::as_str)
                            .unwrap_or("표 크기를 바꿀 수 없습니다");
                        return Err(EngineError(if why.contains("병합") {
                            "합쳐진 셀이 있는 표는 줄 높이를 아직 바꿀 수 없습니다".into()
                        } else {
                            why.to_owned()
                        }));
                    }
                }
                Ok(())
            })?;
        }
        Ok(self.cell_at(pos, info.row, info.col).unwrap_or(pos))
    }

    /// Scale the whole table at `pos` (drag of its bottom-right corner): every column
    /// by `fx` and every row by `fy`, as one undo step. Returns `false` in the second
    /// field when rows could not be scaled (tables with merged cells).
    pub fn scale_table(&mut self, pos: Pos, fx: f64, fy: f64) -> Result<(Pos, bool), EngineError> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = pos
        else {
            return Err(EngineError("캐럿을 표 안에 두세요".into()));
        };
        let info = self
            .cell_info(pos)
            .ok_or_else(|| EngineError("표 셀을 찾을 수 없습니다".into()))?;
        const MIN: f64 = 2.0 * RESIZE_STEP_HWPUNIT;
        let widths: Option<Vec<u32>> = if (fx - 1.0).abs() > 1e-3 {
            let w = self
                .column_widths(pos)
                .ok_or_else(|| EngineError("열 너비를 알 수 없습니다".into()))?;
            Some(
                w.iter()
                    .map(|&x| (x as f64 * fx).max(MIN).round() as u32)
                    .collect(),
            )
        } else {
            None
        };
        // rows: steps per row from the rendered heights
        let merged = self
            .table(pos)
            .is_some_and(|t| t.cells.iter().any(|c| c.row_span > 1 || c.col_span > 1));
        let mut row_steps: Vec<(u16, i32)> = Vec::new();
        if (fy - 1.0).abs() > 1e-3 && !merged {
            let page = self.caret_rect(pos).map(|r| r.page).unwrap_or(0);
            let boxes = self.cell_boxes(pos, page);
            // declared heights can be below the rendered (content) height, so aim for
            // `fy` times what is on screen, measured from what is stored
            let declared = self
                .table(pos)
                .map(|t| t.get_raw_row_heights())
                .unwrap_or_default();
            for r in 0..info.rows {
                if let Some(b) = boxes.iter().find(|b| b.row == r && b.row_span == 1) {
                    let target = b.h * fy * HWPUNIT_PER_PX;
                    let stored = declared.get(r as usize).copied().unwrap_or(0) as f64;
                    let steps = ((target - stored) / RESIZE_STEP_HWPUNIT).round() as i32;
                    if steps != 0 {
                        row_steps.push((r, steps));
                    }
                }
            }
        }
        if widths.is_none() && row_steps.is_empty() {
            return Ok((pos, !(merged && (fy - 1.0).abs() > 1e-3)));
        }
        self.mutate(Step::Other, pos, |core| {
            if let Some(w) = widths {
                core.set_table_column_widths_native(section, parent_para, control, w)
                    .map_err(err)?;
            }
            for &(row, steps) in &row_steps {
                for _ in 0..steps.unsigned_abs() {
                    core.resize_table_native(
                        section,
                        parent_para,
                        control,
                        row,
                        0,
                        true,
                        steps > 0,
                        false,
                    )
                    .map_err(err)?;
                }
            }
            Ok(())
        })?;
        let rows_ok = !(merged && (fy - 1.0).abs() > 1e-3);
        Ok((
            self.cell_at(pos, info.row, info.col).unwrap_or(pos),
            rows_ok,
        ))
    }

    /// Make every column of the table at `pos` the same width (table width unchanged).
    pub fn equalize_columns(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = pos
        else {
            return Err(EngineError("캐럿을 표 안에 두세요".into()));
        };
        let widths = self
            .column_widths(pos)
            .ok_or_else(|| EngineError("열 너비를 알 수 없습니다".into()))?;
        let n = widths.len() as u32;
        if n == 0 {
            return Ok(pos);
        }
        let total: u32 = widths.iter().sum();
        let mut eq = vec![total / n; n as usize];
        eq[n as usize - 1] += total - (total / n) * n; // keep the exact total
        let info = self.cell_info(pos);
        self.mutate(Step::Other, pos, |core| {
            core.set_table_column_widths_native(section, parent_para, control, eq)
                .map_err(err)
        })?;
        Ok(info
            .and_then(|i| self.cell_at(pos, i.row, i.col))
            .unwrap_or(pos))
    }

    /// Make every row of the table at `pos` as tall as the rows' average (rendered) height.
    pub fn equalize_rows(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = pos
        else {
            return Err(EngineError("캐럿을 표 안에 두세요".into()));
        };
        let info = self
            .cell_info(pos)
            .ok_or_else(|| EngineError("표 셀을 찾을 수 없습니다".into()))?;
        if self
            .table(pos)
            .is_some_and(|t| t.cells.iter().any(|c| c.row_span > 1 || c.col_span > 1))
        {
            return Err(EngineError(
                "합쳐진 셀이 있는 표는 줄 높이를 아직 바꿀 수 없습니다".into(),
            ));
        }
        let page = self.caret_rect(pos).map(|r| r.page).unwrap_or(0);
        let boxes = self.cell_boxes(pos, page);
        let heights: Vec<(u16, f64)> = (0..info.rows)
            .filter_map(|r| boxes.iter().find(|b| b.row == r).map(|b| (r, b.h)))
            .collect();
        if heights.is_empty() {
            return Ok(pos);
        }
        let avg = heights.iter().map(|h| h.1).sum::<f64>() / heights.len() as f64;
        let declared = self
            .table(pos)
            .map(|t| t.get_raw_row_heights())
            .unwrap_or_default();
        let steps: Vec<(u16, i32)> = heights
            .iter()
            .map(|&(r, _)| {
                // a row whose content is taller than the average stays at its content height
                let stored = declared.get(r as usize).copied().unwrap_or(0) as f64;
                (
                    r,
                    ((avg * HWPUNIT_PER_PX - stored) / RESIZE_STEP_HWPUNIT).round() as i32,
                )
            })
            .filter(|&(_, n)| n != 0)
            .collect();
        if !steps.is_empty() {
            self.mutate(Step::Other, pos, |core| {
                for &(row, n) in &steps {
                    for _ in 0..n.unsigned_abs() {
                        core.resize_table_native(
                            section,
                            parent_para,
                            control,
                            row,
                            0,
                            true,
                            n > 0,
                            false,
                        )
                        .map_err(err)?;
                    }
                }
                Ok(())
            })?;
        }
        Ok(self.cell_at(pos, info.row, info.col).unwrap_or(pos))
    }

    /// Copy the whole table at `pos` (a cell position) to the internal clipboard.
    pub fn copy_table(&mut self, pos: Pos) -> Result<(), EngineError> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = pos
        else {
            return Err(EngineError("캐럿을 표 안에 두세요".into()));
        };
        self.core
            .copy_control_native(section, parent_para, &[], control)
            .map_err(err)?;
        Ok(())
    }

    /// True when the internal clipboard holds an object (table, picture, shape).
    pub fn clipboard_has_object(&self) -> bool {
        self.core.clipboard_has_control_native()
    }

    /// Paste a copied object at `pos` (body text); the caret goes after it.
    pub fn paste_object(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        let Pos::Body {
            section,
            para,
            offset,
        } = pos
        else {
            return Err(EngineError("표는 본문에만 붙일 수 있습니다".into()));
        };
        let r = self.mutate(Step::Other, pos, |core| {
            core.paste_control_native(section, para, offset)
                .map_err(err)
        })?;
        let v = json(&r)?;
        if v.get("ok").and_then(Value::as_bool) == Some(false) {
            return Err(EngineError("붙일 개체가 없습니다".into()));
        }
        let after = u(&v, "paraIdx").unwrap_or(para);
        // the pasted table(s) sit between where the paste started and the caret after it
        for p in para..=after {
            let n = self.core.document().sections[section]
                .paragraphs
                .get(p)
                .map_or(0, |x| x.controls.len());
            for control in 0..n {
                self.sync_table_flow(Pos::Cell {
                    section,
                    parent_para: p,
                    control,
                    cell: 0,
                    cell_para: 0,
                    offset: 0,
                });
            }
        }
        Ok(Pos::Body {
            section,
            para: after,
            offset: 0,
        })
    }

    /// Remove the whole table; the caret goes to the paragraph that held it.
    pub fn delete_table(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        self.counting_removals(|d| d.remove_table(pos))
    }

    fn remove_table(&mut self, pos: Pos) -> Result<Pos, EngineError> {
        let Pos::Cell {
            section,
            parent_para,
            control,
            ..
        } = pos
        else {
            return Err(EngineError("캐럿을 표 안에 두세요".into()));
        };
        self.mutate(Step::Other, pos, |core| {
            core.delete_table_control_native(section, parent_para, control)
                .map_err(err)
        })?;
        let para = parent_para.min(
            self.core.document().sections[section]
                .paragraphs
                .len()
                .saturating_sub(1),
        );
        Ok(Pos::Body {
            section,
            para,
            offset: 0,
        })
    }

    // ------------------------------------------------------------------ export

    /// All pages as one PDF (rhwp's svg2pdf path, text embedded).
    pub fn export_pdf(&self) -> Result<Vec<u8>, EngineError> {
        let pages: Vec<String> = (0..self.page_count())
            .map(|p| self.render_page_svg(p))
            .collect::<Result<_, _>>()?;
        rhwp::renderer::pdf::svgs_to_pdf_with_options(
            &pages,
            &rhwp::renderer::pdf::PdfExportOptions::default(),
        )
        .map_err(EngineError)
    }

    // ------------------------------------------------------------------ find / replace

    /// Every match of `query` in body text and (non-nested) table cells, in document order.
    pub fn find_all(&self, query: &str, case_sensitive: bool) -> Vec<Range> {
        if query.is_empty() {
            return Vec::new();
        }
        let Ok(s) = self
            .core
            .search_all_text_native(query, case_sensitive, true)
        else {
            return Vec::new();
        };
        let Ok(Value::Array(hits)) = serde_json::from_str::<Value>(&s) else {
            return Vec::new();
        };
        let n = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).map(|x| x as usize);
        hits.iter()
            .filter(|h| h.get("equationControl").is_none() && h.get("cellPath").is_none())
            .filter_map(|h| {
                let (section, offset, len) = (n(h, "sec")?, n(h, "charOffset")?, n(h, "length")?);
                let start = match h.get("cellContext") {
                    Some(c) => Pos::Cell {
                        section,
                        parent_para: n(c, "parentPara")?,
                        control: n(c, "ctrlIdx")?,
                        cell: n(c, "cellIdx")?,
                        cell_para: n(c, "cellPara")?,
                        offset,
                    },
                    None => Pos::Body {
                        section,
                        para: n(h, "para")?,
                        offset,
                    },
                };
                Some(Range {
                    start,
                    end: start.with_offset(offset + len),
                })
            })
            .collect()
    }

    /// Replace every match as one undo step; returns how many were replaced.
    pub fn replace_all(
        &mut self,
        query: &str,
        with: &str,
        case_sensitive: bool,
    ) -> Result<usize, EngineError> {
        let hits = self.find_all(query, case_sensitive);
        if hits.is_empty() {
            return Ok(0);
        }
        let caret = hits[0].start;
        self.mutate(Step::Other, caret, |core| {
            // back to front, so earlier offsets stay valid
            for r in hits.iter().rev() {
                Self::delete_range_in(core, *r)?;
                if !with.is_empty() {
                    insert_in(core, r.start, with)?;
                }
            }
            Ok(hits.len())
        })
    }

    /// Highlight rectangles for `r`, one per line run (caret rects grouped by line).
    pub fn selection_rects(&self, r: Range) -> Vec<SelRect> {
        let (first, last) = (para_of(r.start), para_of(r.end));
        if first == last {
            return self.selection_rects_native(r);
        }
        let whole = |i: usize| {
            let p = with_para(r.start, i).with_offset(0);
            Range {
                start: p,
                end: p.with_offset(self.para_len(p)),
            }
        };
        let mut out = self.selection_rects_native(Range {
            start: r.start,
            end: whole(first).end,
        });
        {
            let mut cache = self.sel_cache.borrow_mut();
            if cache.0 != self.gen {
                *cache = (self.gen, std::collections::HashMap::new());
            }
            for i in first + 1..last {
                let w = whole(i);
                let rects = cache.1.entry(w.start).or_insert_with(|| {
                    // caret queries keep the page's layout tree cached, which makes the
                    // rectangle lookup ~40x cheaper than building the tree for it
                    let _ = self.caret_rect(w.start);
                    self.selection_rects_native(w)
                });
                out.extend_from_slice(rects);
            }
        }
        out.extend(self.selection_rects_native(Range {
            start: whole(last).start,
            end: r.end,
        }));
        out
    }

    /// One call to rhwp's line-by-line selection rectangles; the per-character
    /// fallback if rhwp cannot give them.
    fn selection_rects_native(&self, r: Range) -> Vec<SelRect> {
        // rhwp works line by line from the laid-out pages (patch 0012)
        let (section, ctx) = match r.start {
            Pos::Body { section, .. } => (section, None),
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                ..
            } => (section, Some((parent_para, control, cell))),
        };
        let native = self
            .core
            .get_selection_rects_native(
                section,
                para_of(r.start),
                r.start.offset(),
                para_of(r.end),
                r.end.offset(),
                ctx,
                None,
            )
            .ok()
            .and_then(|s| json(&s).ok())
            .and_then(|v| {
                v.as_array()?
                    .iter()
                    .map(|o| {
                        let f = |k: &str| o.get(k).and_then(Value::as_f64);
                        Some(SelRect {
                            page: o.get("pageIndex")?.as_u64()? as u32,
                            x: f("x")?,
                            y: f("y")?,
                            w: f("width")?,
                            h: f("height")?,
                        })
                    })
                    .collect::<Option<Vec<_>>>()
            });
        match native {
            Some(mut rects) => {
                rects.retain(|r| r.w > 0.0);
                rects
            }
            None => self.selection_rects_by_caret(r),
        }
    }

    /// Selection rectangles from the caret position of every character (the fallback
    /// when rhwp cannot give them by line); stops after 4000 characters.
    fn selection_rects_by_caret(&self, r: Range) -> Vec<SelRect> {
        const LIMIT: usize = 4000;
        let mut out: Vec<SelRect> = Vec::new();
        let mut budget = LIMIT;
        for i in para_of(r.start)..=para_of(r.end) {
            let p = with_para(r.start, i);
            let s = if i == para_of(r.start) {
                r.start.offset()
            } else {
                0
            };
            let e = if i == para_of(r.end) {
                r.end.offset()
            } else {
                self.para_len(p)
            };
            for o in s..=e {
                if budget == 0 {
                    return out;
                }
                budget -= 1;
                let Ok(c) = self.caret_rect(p.with_offset(o)) else {
                    continue;
                };
                match out.last_mut() {
                    Some(l) if l.page == c.page && (l.y - c.y).abs() < 0.5 && c.x >= l.x => {
                        l.w = (c.x - l.x).max(l.w);
                        l.h = l.h.max(c.height);
                    }
                    _ => out.push(SelRect {
                        page: c.page,
                        x: c.x,
                        y: c.y,
                        w: 0.0,
                        h: c.height,
                    }),
                }
            }
        }
        out.retain(|r| r.w > 0.0);
        out
    }

    /// Serialize to `format`. Fails if rhwp reports any content loss.
    pub fn serialize(&self, format: Format) -> Result<Serialized, EngineError> {
        let out = match format {
            Format::Hwp => self.core.export_hwp_with_adapter_snapshot_with_report(),
            Format::Hwpx => self.core.export_hwpx_native_with_report(),
        }
        .map_err(err)?;
        let (bytes, report) = out.into_parts();
        if !report.is_empty() {
            return Err(EngineError(format!(
                "저장하면 내용 {}건이 손실되어 저장을 중단했습니다: {}",
                report.len(),
                report.to_json()
            )));
        }
        Ok(Serialized(bytes))
    }

    /// Re-parse `bytes` and check they describe a document with pages.
    pub fn validate(bytes: &[u8]) -> Result<(), String> {
        let d = DocumentCore::from_bytes(bytes).map_err(|e| e.to_string())?;
        if d.page_count() == 0 {
            return Err("다시 연 문서의 쪽 수가 0입니다".into());
        }
        Ok(())
    }

    /// Number of sections and characters (without paragraph breaks), for the status bar.
    pub fn counts(&self) -> (usize, usize) {
        let chars = self.all_text().chars().filter(|c| *c != '\n').count();
        (self.core.document().sections.len(), chars)
    }

    pub fn section_para_counts(&self) -> Vec<usize> {
        self.core
            .document()
            .sections
            .iter()
            .map(|s| s.paragraphs.len())
            .collect()
    }

    /// Text of body paragraphs and (recursively) table cells, one paragraph per line,
    /// in document order: what a word processor's "save as text" would contain.
    pub fn all_text(&self) -> String {
        fn walk(paras: &[rhwp::model::paragraph::Paragraph], out: &mut String) {
            for p in paras {
                out.push_str(&p.text);
                out.push('\n');
                for c in &p.controls {
                    if let rhwp::model::control::Control::Table(t) = c {
                        for cell in &t.cells {
                            walk(&cell.paragraphs, out);
                        }
                    }
                }
            }
        }
        let mut out = String::new();
        for s in &self.core.document().sections {
            walk(&s.paragraphs, &mut out);
        }
        out
    }

    /// Plain text of every body paragraph, for round-trip comparison.
    pub fn body_text(&self) -> Vec<String> {
        self.core
            .document()
            .sections
            .iter()
            .flat_map(|s| s.paragraphs.iter().map(|p| p.text.clone()))
            .collect()
    }
}

fn insert_in(core: &mut DocumentCore, pos: Pos, text: &str) -> Result<String, EngineError> {
    match pos {
        Pos::Body {
            section,
            para,
            offset,
        } => core.insert_text_native(section, para, offset, text),
        Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            cell_para,
            offset,
        } => core.insert_text_in_cell_native(
            section,
            parent_para,
            control,
            cell,
            cell_para,
            offset,
            text,
        ),
    }
    .map_err(err)
}

fn split_in(core: &mut DocumentCore, pos: Pos) -> Result<Pos, EngineError> {
    let r = match pos {
        Pos::Body {
            section,
            para,
            offset,
        } => core.split_paragraph_native(section, para, offset, None),
        Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            cell_para,
            offset,
        } => core.split_paragraph_in_cell_native(
            section,
            parent_para,
            control,
            cell,
            cell_para,
            offset,
            None,
        ),
    }
    .map_err(err)?;
    let v = json(&r)?;
    Ok(match pos {
        Pos::Body { section, .. } => Pos::Body {
            section,
            para: u(&v, "paraIdx")?,
            offset: 0,
        },
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
            cell_para: u(&v, "cellParaIndex")?,
            offset: 0,
        },
    })
}

/// `lens[k]` = length of the k-th paragraph of `r` (only interior/last-but-start ends use it).
fn apply_char_format_in(
    core: &mut DocumentCore,
    r: Range,
    lens: &[usize],
    fmt: &CharFormat,
) -> Result<(), EngineError> {
    let mut props = serde_json::Map::new();
    let mut put = |k: &str, v: Value| {
        props.insert(k.into(), v);
    };
    if let Some(b) = fmt.bold {
        put("bold", b.into());
    }
    if let Some(b) = fmt.italic {
        put("italic", b.into());
    }
    if let Some(b) = fmt.underline {
        put("underline", b.into());
    }
    if let Some(b) = fmt.strike {
        put("strikethrough", b.into());
    }
    if let Some(pt) = fmt.size_pt {
        put("fontSize", ((pt * 100.0).round() as i64).into());
    }
    if let Some(c) = &fmt.color {
        put("textColor", c.clone().into());
    }
    if let Some(name) = &fmt.font {
        let id = core.find_or_create_font_id_native(name);
        if id < 0 {
            return Err(EngineError(format!("글꼴을 등록할 수 없습니다: {name}")));
        }
        put("fontId", id.into());
    }
    let props = Value::Object(props).to_string();
    for (k, i) in (para_of(r.start)..=para_of(r.end)).enumerate() {
        let s = if i == para_of(r.start) {
            r.start.offset()
        } else {
            0
        };
        let e = if i == para_of(r.end) {
            r.end.offset()
        } else {
            lens[k]
        };
        if s >= e {
            continue;
        }
        match r.start {
            Pos::Body { section, .. } => core.apply_char_format_native(section, i, s, e, &props),
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                ..
            } => core.apply_char_format_in_cell_native(
                section,
                parent_para,
                control,
                cell,
                i,
                s,
                e,
                &props,
            ),
        }
        .map_err(err)?;
    }
    Ok(())
}

fn json(s: &str) -> Result<Value, EngineError> {
    serde_json::from_str(s).map_err(|e| EngineError(format!("엔진 응답 해석 실패: {e}: {s}")))
}

/// Apply paragraph properties to paragraph `i` of the container holding `at`.
fn apply_para_props(
    core: &mut DocumentCore,
    at: Pos,
    i: usize,
    props: &str,
) -> Result<(), EngineError> {
    match at {
        Pos::Body { section, .. } => core.apply_para_format_native(section, i, props),
        Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            ..
        } => core.apply_para_format_in_cell_native(section, parent_para, control, cell, i, props),
    }
    .map_err(err)?;
    sync_head_tag(core, at, i);
    Ok(())
}

fn apply_style_at(
    core: &mut DocumentCore,
    at: Pos,
    i: usize,
    style: usize,
) -> Result<(), EngineError> {
    match at {
        Pos::Body { section, .. } => core.apply_style_native(section, i, style),
        Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            ..
        } => core.apply_cell_style_native(section, parent_para, control, cell, i, style),
    }
    .map_err(err)?;
    sync_head_tag(core, at, i);
    Ok(())
}

/// Apply rhwp paragraph properties (JSON) to every paragraph touched by `r`.
fn para_props_in(core: &mut DocumentCore, r: Range, props: &str) -> Result<(), EngineError> {
    for i in para_of(r.start)..=para_of(r.end) {
        match r.start {
            Pos::Body { section, .. } => core.apply_para_format_native(section, i, props),
            Pos::Cell {
                section,
                parent_para,
                control,
                cell,
                ..
            } => core.apply_para_format_in_cell_native(
                section,
                parent_para,
                control,
                cell,
                i,
                props,
            ),
        }
        .map_err(err)?;
    }
    Ok(())
}

/// Hwp marks the first line of a paragraph that has a head (번호·글머리표·개요) with
/// LINE_SEG bit 21 (문단 머리 모양 적용), and Hwp 2022 does not draw a bullet whose
/// stored line lacks it. rhwp's reflow keeps a line's earlier tag, so setting it here
/// lasts through later edits.
fn sync_head_tag(core: &mut DocumentCore, at: Pos, i: usize) {
    use rhwp::model::control::Control;
    use rhwp::model::paragraph::LineSeg;
    use rhwp::model::style::HeadType;
    let doc = core.document_mut();
    let (info, sections) = (&doc.doc_info, &mut doc.sections);
    let para = match at {
        Pos::Body { section, .. } => sections
            .get_mut(section)
            .and_then(|s| s.paragraphs.get_mut(i)),
        Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            ..
        } => {
            match sections
                .get_mut(section)
                .and_then(|s| s.paragraphs.get_mut(parent_para))
                .and_then(|p| p.controls.get_mut(control))
            {
                Some(Control::Table(t)) => {
                    t.cells.get_mut(cell).and_then(|c| c.paragraphs.get_mut(i))
                }
                _ => None,
            }
        }
    };
    let Some(para) = para else { return };
    let head = info
        .para_shapes
        .get(para.para_shape_id as usize)
        .is_some_and(|s| s.head_type != HeadType::None);
    if let Some(first) = para.line_segs.first_mut() {
        if head {
            first.tag |= LineSeg::TAG_PARAGRAPH_HEAD;
        } else {
            first.tag &= !LineSeg::TAG_PARAGRAPH_HEAD;
        }
    }
}

fn para_props(core: &DocumentCore, at: Pos, i: usize) -> Result<String, EngineError> {
    match at {
        Pos::Body { section, .. } => core.get_para_properties_at_native(section, i),
        Pos::Cell {
            section,
            parent_para,
            control,
            cell,
            ..
        } => core.get_cell_para_properties_at_native(section, parent_para, control, cell, i),
    }
    .map_err(err)
}

fn list_state(v: &Value) -> ListState {
    let kind = match v.get("headType").and_then(Value::as_str).unwrap_or("None") {
        "Outline" => ListKind::Outline,
        "Number" => ListKind::Number,
        "Bullet" => ListKind::Bullet,
        _ => ListKind::None,
    };
    ListState {
        kind,
        level: v
            .get("paraLevel")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(6) as u8,
    }
}

/// Hwp's level formats for 개요 and 문단 번호: 1. 가. 1) 가) (1) (가) ①
const HWP_NUMBER_FORMATS: [(&str, u8); 7] = [
    ("^1.", 0),
    ("^2.", 8),
    ("^3)", 0),
    ("^4)", 8),
    ("(^5)", 0),
    ("(^6)", 8),
    ("^7", 1),
];

/// The numbering shape for 문단 번호 in `section` (1-based id). Hwp keeps it apart from
/// the section's 개요 shape: in Hwp-made files it is a copy of the 개요 shape with
/// start 1 (the 개요 one has 0), so numbered paragraphs count on their own. An existing
/// such shape is reused; otherwise one is added the same way.
fn ensure_numbering(core: &mut DocumentCore, section: usize) -> u16 {
    use rhwp::model::style::{Numbering, NumberingHead};
    let doc = core.document_mut();
    let outline = doc
        .sections
        .get(section)
        .map_or(0, |s| s.section_def.outline_numbering_id as usize);
    let hwp_formats = |n: &Numbering| {
        n.level_formats
            .iter()
            .zip(HWP_NUMBER_FORMATS)
            .all(|(f, (h, _))| f == h)
    };
    let numberings = &doc.doc_info.numberings;
    if let Some(i) =
        (0..numberings.len()).find(|&i| i + 1 != outline && hwp_formats(&numberings[i]))
    {
        return (i + 1) as u16;
    }
    let mut n = outline
        .checked_sub(1)
        .and_then(|i| numberings.get(i))
        .cloned()
        .unwrap_or_else(|| {
            let mut n = Numbering::default();
            for (i, (format, code)) in HWP_NUMBER_FORMATS.into_iter().enumerate() {
                n.level_formats[i] = format.to_owned();
                n.heads[i] = NumberingHead {
                    number_format: code,
                    attr: 1 << 3,
                    ..Default::default()
                };
            }
            n.level_start_numbers = [1; 7];
            n
        });
    n.raw_data = None;
    n.start_number = 1;
    doc.doc_info.numberings.push(n);
    doc.doc_info.numberings.len() as u16
}

/// The bullet shape for `ch` (1-based id), added if the document has none.
fn ensure_bullet(core: &mut DocumentCore, ch: char) -> u16 {
    use rhwp::model::style::Bullet;
    let doc = core.document_mut();
    let found = doc
        .doc_info
        .bullets
        .iter()
        .position(|b| rhwp::renderer::layout::map_pua_bullet_char(b.bullet_char) == ch);
    let i = found.unwrap_or_else(|| {
        // no own char shape (u32::MAX), as in every bullet Hwp writes
        doc.doc_info.bullets.push(Bullet {
            bullet_char: ch,
            attr: 1 << 3,
            text_distance: 50,
            char_shape_id: u32::MAX,
            ..Default::default()
        });
        doc.doc_info.bullets.len() - 1
    });
    (i + 1) as u16
}

fn u(v: &Value, k: &str) -> Result<usize, EngineError> {
    v.get(k)
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .ok_or_else(|| EngineError(format!("응답에 {k} 없음")))
}

fn f(v: &Value, k: &str) -> Result<f64, EngineError> {
    v.get(k)
        .and_then(Value::as_f64)
        .ok_or_else(|| EngineError(format!("응답에 {k} 없음")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> Document {
        let p = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../vendor/rhwp/saved/blank2010.hwp"
        );
        Document::from_bytes(&std::fs::read(p).unwrap(), Format::Hwp).unwrap()
    }

    /// Raw WCHAR position of the first text character of body paragraph 0.
    fn first_char_raw(d: &Document) -> u32 {
        d.core.document().sections[0].paragraphs[0].char_offsets[0]
    }

    /// Regression (found with Hwp 2022): typing at the very start of a paragraph that
    /// begins with secd/cold followed by an inline object put the text before secd,
    /// and Hancom then dropped the section's page setup (landscape -> portrait).
    #[test]
    fn typing_at_document_start_keeps_section_definition_first() {
        let mut d = blank();
        let start = Pos::Body {
            section: 0,
            para: 0,
            offset: 0,
        };
        let leading = d.core.document().sections[0].paragraphs[0].controls.len() as u32;
        assert!(leading >= 2, "blank template starts with secd + cold");
        // an inline object at text position 0, like the drawing objects in the failing file
        d.create_table(start, 1, 1).unwrap();
        d.insert_text(start, "앞").unwrap();
        let p = &d.core.document().sections[0].paragraphs[0];
        assert_eq!(p.text.chars().next(), Some('앞'));
        assert!(
            first_char_raw(&d) >= leading * 8,
            "text must follow the {leading} leading controls (raw pos {})",
            first_char_raw(&d)
        );
        // and it survives a save/reopen
        let back = Document::from_bytes(&d.serialize(Format::Hwp).unwrap().0, Format::Hwp).unwrap();
        assert!(first_char_raw(&back) >= leading * 8);
    }
}
