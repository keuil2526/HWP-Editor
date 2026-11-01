//! Format detection by content, never by file extension.
//!
//! Local survey found `.hwp` files that are really HWPX archives, and macOS
//! AppleDouble (`._*`) files carrying a `.hwp` extension.

const CFB_MAGIC: &[u8; 8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";
const ZIP_MAGIC: &[u8; 4] = b"PK\x03\x04";
const HWPX_MIMETYPE: &[u8] = b"application/hwp+zip";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// HWP 5.x binary (OLE compound file).
    Hwp,
    /// HWPX / OWPML (ZIP + XML).
    Hwpx,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Hwp => "hwp",
            Format::Hwpx => "hwpx",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectError {
    TooShort,
    /// macOS resource-fork sidecar, not a document.
    AppleDouble,
    /// ZIP archive whose first entry is not the HWPX `mimetype`.
    ZipNotHwpx,
    Unknown([u8; 8]),
}

impl std::fmt::Display for DetectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetectError::TooShort => write!(f, "파일이 너무 짧아 HWP/HWPX 문서가 아닙니다"),
            DetectError::AppleDouble => write!(f, "macOS 보조 파일(._*)이며 문서가 아닙니다"),
            DetectError::ZipNotHwpx => write!(f, "ZIP 파일이지만 HWPX 문서가 아닙니다"),
            DetectError::Unknown(m) => write!(f, "알 수 없는 형식입니다 (시그니처 {:02x?})", m),
        }
    }
}

impl std::error::Error for DetectError {}

pub fn detect(bytes: &[u8]) -> Result<Format, DetectError> {
    if bytes.len() < 8 {
        return Err(DetectError::TooShort);
    }
    if &bytes[..8] == CFB_MAGIC {
        // Further HWP checks (FileHeader signature) belong to the parser.
        return Ok(Format::Hwp);
    }
    if &bytes[..4] == ZIP_MAGIC {
        return if zip_first_entry_is_hwpx_mimetype(bytes) {
            Ok(Format::Hwpx)
        } else {
            Err(DetectError::ZipNotHwpx)
        };
    }
    if bytes[..4] == [0x00, 0x05, 0x16, 0x07] {
        return Err(DetectError::AppleDouble);
    }
    let mut m = [0u8; 8];
    m.copy_from_slice(&bytes[..8]);
    Err(DetectError::Unknown(m))
}

/// Local file header: name at offset 30, data after name + extra field.
fn zip_first_entry_is_hwpx_mimetype(b: &[u8]) -> bool {
    if b.len() < 30 {
        return false;
    }
    let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
    let method = u16_at(8);
    let name_len = u16_at(26);
    let extra_len = u16_at(28);
    let name_end = 30 + name_len;
    let data = name_end + extra_len;
    if b.len() < data + HWPX_MIMETYPE.len() || &b[30..name_end] != b"mimetype" {
        return false;
    }
    method == 0 && b[data..].starts_with(HWPX_MIMETYPE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_with_first(name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(ZIP_MAGIC);
        v.extend_from_slice(&[20, 0, 0, 0, 0, 0]); // version, flags, method=stored
        v.extend_from_slice(&[0; 16]); // time, date, crc, sizes
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(name);
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn detects_cfb_as_hwp() {
        let mut b = CFB_MAGIC.to_vec();
        b.extend_from_slice(&[0; 504]);
        assert_eq!(detect(&b), Ok(Format::Hwp));
    }

    #[test]
    fn detects_hwpx_regardless_of_extension() {
        let b = zip_with_first(b"mimetype", b"application/hwp+zip");
        assert_eq!(detect(&b), Ok(Format::Hwpx));
    }

    #[test]
    fn rejects_other_zip() {
        let b = zip_with_first(b"word/document.xml", b"<xml/>");
        assert_eq!(detect(&b), Err(DetectError::ZipNotHwpx));
    }

    #[test]
    fn rejects_appledouble() {
        let b = [0x00, 0x05, 0x16, 0x07, 0x00, 0x02, 0x00, 0x00, 0, 0];
        assert_eq!(detect(&b), Err(DetectError::AppleDouble));
    }
}
