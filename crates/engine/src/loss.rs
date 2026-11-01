//! Loss guard: compares what a file contains before and after serialization.
//!
//! rhwp re-creates an edited section from its model; anything the model does
//! not hold is silently dropped (found with Hwp 2022: HWP5 memo lists, which
//! made Hwp crash on the saved file). Independently of rhwp's own content-loss
//! report, this counts record kinds (HWP), XML element kinds (HWPX) and binary
//! data entries, and lists every kind whose count went down that the editor
//! cannot remove on purpose.

use hwpedit_core::Format;
use std::collections::BTreeMap;
use std::io::Read;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loss {
    /// Human-readable kind, e.g. "메모 목록".
    pub what: String,
    pub before: usize,
    pub after: usize,
}

impl std::fmt::Display for Loss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}개 → {}개", self.what, self.before, self.after)
    }
}

/// HWP body records that text, paragraph and table editing create and remove.
const HWP_EDITABLE: [u16; 8] = [66, 67, 68, 69, 70, 71, 72, 77];

/// HWPX section elements that text, paragraph and table editing create and remove.
const HWPX_EDITABLE: [&str; 17] = [
    "hp:p",
    "hp:run",
    "hp:t",
    "hp:linesegarray",
    "hp:lineseg",
    "hp:tbl",
    "hp:tr",
    "hp:tc",
    "hp:subList",
    "hp:cellAddr",
    "hp:cellSpan",
    "hp:cellSz",
    "hp:cellMargin",
    "hp:sz",
    "hp:pos",
    "hp:outMargin",
    "hp:inMargin",
];

fn hwp_tag_name(tag: u16) -> String {
    let n = match tag {
        18 => "그림·첨부 정보",
        19 => "글꼴",
        20 => "테두리·배경",
        21 => "글자 모양",
        22 => "탭 정의",
        23 => "문단 번호",
        24 => "글머리표",
        25 => "문단 모양",
        26 => "스타일",
        73 => "쪽 설정",
        74 => "각주·미주 모양",
        75 => "쪽 테두리",
        76 => "그리기 개체",
        78..=83 | 86 => "도형",
        84 => "OLE 개체",
        85 => "그림",
        87 => "개체 데이터",
        88 => "수식",
        90 => "글맵시",
        91 => "양식 개체",
        92 => "메모 모양",
        93 => "메모 목록",
        95 => "차트",
        _ => return format!("HWP 레코드 {tag}"),
    };
    format!("{n} (레코드 {tag})")
}

fn hwpx_name(el: &str) -> String {
    let n = match el {
        "hp:pic" => "그림",
        "hp:equation" => "수식",
        "hp:rect" | "hp:line" | "hp:ellipse" | "hp:arc" | "hp:polygon" | "hp:curve"
        | "hp:connectLine" | "hp:container" => "도형",
        "hp:ole" => "OLE 개체",
        "hp:chart" => "차트",
        "hp:footNote" | "hp:endNote" => "각주·미주",
        "hp:header" | "hp:footer" => "머리말·꼬리말",
        "hp:fieldBegin" | "hp:fieldEnd" => "필드",
        "hp:memo" | "hp:memogroup" => "메모",
        "hp:secPr" | "hp:pagePr" => "구역·쪽 설정",
        _ => return format!("요소 <{el}>"),
    };
    format!("{n} <{el}>")
}

/// Kind -> count. Keys are prefixed by the part they come from.
pub type Inventory = BTreeMap<String, usize>;

/// Count every record kind (HWP) or element (HWPX) in `bytes`.
pub fn inventory(bytes: &[u8], format: Format) -> Result<Inventory, String> {
    match format {
        Format::Hwp => hwp_inventory(bytes),
        Format::Hwpx => hwpx_inventory(bytes),
    }
}

fn hwp_inventory(bytes: &[u8]) -> Result<BTreeMap<String, usize>, String> {
    let mut cfb =
        cfb::CompoundFile::open(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let read = |cfb: &mut cfb::CompoundFile<_>, path: &str| -> Result<Vec<u8>, String> {
        let mut s = cfb.open_stream(path).map_err(|e| format!("{path}: {e}"))?;
        let mut v = Vec::new();
        s.read_to_end(&mut v).map_err(|e| e.to_string())?;
        Ok(v)
    };
    let header = read(&mut cfb, "/FileHeader")?;
    let compressed = header.get(36).is_some_and(|f| f & 1 == 1);
    let mut streams: Vec<String> = Vec::new();
    let mut bin = 0;
    for e in cfb.walk() {
        let p = e.path().to_string_lossy().replace('\\', "/");
        if e.is_stream() {
            if p.starts_with("/BodyText/Section") || p == "/DocInfo" {
                streams.push(p);
            } else if p.starts_with("/BinData/") {
                bin += 1;
            }
        }
    }
    let mut inv = BTreeMap::new();
    inv.insert("bin:첨부 데이터 (BinData)".into(), bin);
    for p in streams {
        let raw = read(&mut cfb, &p)?;
        let data = if compressed {
            let mut out = Vec::new();
            flate2::read::DeflateDecoder::new(&raw[..])
                .read_to_end(&mut out)
                .map_err(|e| format!("{p}: {e}"))?;
            out
        } else {
            raw
        };
        let part = if p == "/DocInfo" { "docinfo" } else { "body" };
        let mut i = 0;
        while i + 4 <= data.len() {
            let h = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            let (tag, mut size) = ((h & 0x3FF) as u16, (h >> 20) as usize);
            i += 4;
            if size == 0xFFF && i + 4 <= data.len() {
                size =
                    u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
                i += 4;
            }
            i += size;
            *inv.entry(format!("{part}:{tag}")).or_insert(0) += 1;
        }
    }
    Ok(inv)
}

fn hwpx_inventory(bytes: &[u8]) -> Result<BTreeMap<String, usize>, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut inv = BTreeMap::new();
    let names: Vec<String> = zip.file_names().map(str::to_owned).collect();
    inv.insert(
        "bin:첨부 데이터 (BinData)".into(),
        names.iter().filter(|n| n.starts_with("BinData/")).count(),
    );
    for n in names {
        let part = if n.starts_with("Contents/section") && n.ends_with(".xml") {
            "section"
        } else if n == "Contents/header.xml" {
            "header"
        } else {
            continue;
        };
        let mut xml = Vec::new();
        zip.by_name(&n)
            .map_err(|e| e.to_string())?
            .read_to_end(&mut xml)
            .map_err(|e| e.to_string())?;
        for el in start_tags(&xml) {
            *inv.entry(format!("{part}:{el}")).or_insert(0) += 1;
        }
    }
    Ok(inv)
}

/// Names of start tags (`<a:b ...>`), skipping end tags, comments and declarations.
fn start_tags(xml: &[u8]) -> impl Iterator<Item = String> + '_ {
    let mut i = 0;
    std::iter::from_fn(move || {
        while i < xml.len() {
            if xml[i] == b'<' && i + 1 < xml.len() && !matches!(xml[i + 1], b'/' | b'?' | b'!') {
                let s = i + 1;
                let mut e = s;
                while e < xml.len() && !matches!(xml[e], b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>')
                {
                    e += 1;
                }
                i = e;
                return Some(String::from_utf8_lossy(&xml[s..e]).into_owned());
            }
            i += 1;
        }
        None
    })
}

/// What `after` has fewer of than `before`, kind by kind.
pub fn removed_between(before: &Inventory, after: &Inventory) -> Inventory {
    before
        .iter()
        .filter_map(|(k, &n)| {
            n.checked_sub(after.get(k).copied().unwrap_or(0))
                .filter(|&d| d > 0)
                .map(|d| (k.clone(), d))
        })
        .collect()
}

/// Kinds that are fewer in `new` than in `orig` and that editing cannot remove on purpose.
pub fn detect_losses(orig: &[u8], new: &[u8], format: Format) -> Result<Vec<Loss>, String> {
    detect_losses_allowing(orig, new, format, &Inventory::new())
}

/// Like [`detect_losses`], but up to `allowed[kind]` fewer is expected: objects the
/// user deleted (a table with its pictures, say) are not a loss.
pub fn detect_losses_allowing(
    orig: &[u8],
    new: &[u8],
    format: Format,
    allowed: &Inventory,
) -> Result<Vec<Loss>, String> {
    let (a, b) = (inventory(orig, format)?, inventory(new, format)?);
    let mut out = Vec::new();
    for (key, &before) in &a {
        let after = b.get(key).copied().unwrap_or(0);
        if after + allowed.get(key).copied().unwrap_or(0) >= before {
            continue;
        }
        let (part, kind) = key.split_once(':').unwrap_or(("", key));
        let what = match (format, part) {
            (_, "bin") => kind.to_owned(),
            (Format::Hwp, "body") => {
                let tag: u16 = kind.parse().unwrap_or(0);
                if HWP_EDITABLE.contains(&tag) {
                    continue;
                }
                hwp_tag_name(tag)
            }
            (Format::Hwp, _) => format!("문서 정보: {}", hwp_tag_name(kind.parse().unwrap_or(0))),
            (Format::Hwpx, "section") => {
                if HWPX_EDITABLE.contains(&kind) {
                    continue;
                }
                hwpx_name(kind)
            }
            (Format::Hwpx, _) => format!("문서 정보: <{kind}>"),
        };
        out.push(Loss {
            what,
            before,
            after,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_tag_scan() {
        let x = br#"<?xml version="1.0"?><hs:sec a="1"><hp:p><hp:t>x</hp:t><hp:pic/></hp:p><!-- c --></hs:sec>"#;
        let v: Vec<String> = start_tags(x).collect();
        assert_eq!(v, ["hs:sec", "hp:p", "hp:t", "hp:pic"]);
    }

    fn hwpx(section: &str, header: &str, bins: usize) -> Vec<u8> {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut z = zip::ZipWriter::new(&mut buf);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        z.start_file("mimetype", stored).unwrap();
        z.write_all(b"application/hwp+zip").unwrap();
        z.start_file("Contents/header.xml", stored).unwrap();
        z.write_all(header.as_bytes()).unwrap();
        z.start_file("Contents/section0.xml", stored).unwrap();
        z.write_all(section.as_bytes()).unwrap();
        for i in 0..bins {
            z.start_file(format!("BinData/image{i}.png"), stored)
                .unwrap();
            z.write_all(b"x").unwrap();
        }
        z.finish().unwrap();
        buf.into_inner()
    }

    #[test]
    fn hwpx_losses_ignore_edited_kinds_and_report_the_rest() {
        let a = hwpx(
            "<hs:sec><hp:p><hp:t>a</hp:t><hp:pic/></hp:p><hp:p><hp:pic/></hp:p></hs:sec>",
            "<hh:head><hp:switch/></hh:head>",
            2,
        );
        // one paragraph deleted (fine), one picture and its data gone, header switch dropped
        let b = hwpx(
            "<hs:sec><hp:p><hp:t>a</hp:t><hp:pic/></hp:p></hs:sec>",
            "<hh:head/>",
            1,
        );
        let l = detect_losses(&a, &b, Format::Hwpx).unwrap();
        let what: Vec<_> = l.iter().map(|x| x.what.as_str()).collect();
        assert_eq!(
            what,
            [
                "첨부 데이터 (BinData)",
                "문서 정보: <hp:switch>",
                "그림 <hp:pic>"
            ],
            "{l:?}"
        );
        assert_eq!((l[2].before, l[2].after), (2, 1));
        assert!(
            detect_losses(&b, &a, Format::Hwpx).unwrap().is_empty(),
            "additions are not losses"
        );
    }

    #[test]
    fn allowance_covers_only_what_was_deleted() {
        let a = hwpx(
            "<hs:sec><hp:p><hp:pic/><hp:pic/><hp:equation/></hp:p></hs:sec>",
            "<hh:head/>",
            0,
        );
        let b = hwpx("<hs:sec><hp:p></hp:p></hs:sec>", "<hh:head/>", 0);
        let allowed = Inventory::from([("section:hp:pic".to_owned(), 1)]);
        let l = detect_losses_allowing(&a, &b, Format::Hwpx, &allowed).unwrap();
        let what: Vec<_> = l.iter().map(|x| x.what.as_str()).collect();
        assert_eq!(
            what,
            ["수식 <hp:equation>", "그림 <hp:pic>"],
            "one picture more than deleted, and the equation: {l:?}"
        );
        let both = Inventory::from([
            ("section:hp:pic".to_owned(), 2),
            ("section:hp:equation".to_owned(), 1),
        ]);
        assert!(detect_losses_allowing(&a, &b, Format::Hwpx, &both)
            .unwrap()
            .is_empty());
        assert_eq!(
            removed_between(
                &inventory(&a, Format::Hwpx).unwrap(),
                &inventory(&b, Format::Hwpx).unwrap()
            )["section:hp:pic"],
            2
        );
    }

    #[test]
    fn identical_files_have_no_losses() {
        let b = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../vendor/rhwp/saved/blank2010.hwp"
        ))
        .unwrap();
        assert!(detect_losses(&b, &b, Format::Hwp).unwrap().is_empty());
        assert!(inventory(&b, Format::Hwp)
            .unwrap()
            .keys()
            .any(|k| k == "body:66"));
    }
}
