//! Exports a protocol (Markdown from the summary editor) to a Word document.
//!
//! Supports the Markdown the protocol templates produce: headings, paragraphs,
//! bullet and numbered lists, **bold**, and pipe tables (доручення). The page is
//! A4 with Times New Roman 14 pt, the usual setting for official documents.

use std::io::Write;
use std::path::PathBuf;

use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;
use zip::write::SimpleFileOptions;

const FONT: &str = "Times New Roman";
/// Half-points: 28 = 14 pt
const BODY_SIZE: u32 = 28;

/// Asks where to save, writes the DOCX, and returns the path (None when cancelled).
#[tauri::command]
pub async fn api_export_protocol_docx<R: Runtime>(
    app: AppHandle<R>,
    title: String,
    markdown: String,
) -> Result<Option<String>, String> {
    // The marks of where points come from are for the app only
    let markdown = crate::summary::materials::without_source_marks(&markdown);
    let file_name = format!("{}.docx", sanitize_file_name(&title));
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name(&file_name)
            .add_filter("Документ Word", &["docx"])
            .blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;

    let Some(picked) = picked else {
        return Ok(None);
    };
    let mut path: PathBuf = picked.into_path().map_err(|e| e.to_string())?;
    if path.extension().map_or(true, |ext| ext != "docx") {
        path.set_extension("docx");
    }

    let bytes = build_docx(&markdown).map_err(|e| format!("Не вдалося створити DOCX: {}", e))?;
    std::fs::write(&path, bytes).map_err(|e| format!("Не вдалося зберегти файл: {}", e))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

pub(crate) fn sanitize_file_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() { "Протокол".to_string() } else { trimmed.to_string() }
}

pub fn build_docx(markdown: &str) -> zip::result::ZipResult<Vec<u8>> {
    let body = markdown_to_body(markdown);
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1134" w:right="567" w:bottom="1134" w:left="1701" w:header="709" w:footer="709" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{FONT}" w:hAnsi="{FONT}" w:cs="{FONT}" w:eastAsia="{FONT}"/><w:sz w:val="{BODY_SIZE}"/><w:szCs w:val="{BODY_SIZE}"/><w:lang w:val="uk-UA"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults></w:styles>"#
    );

    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("[Content_Types].xml", options)?;
        zip.write_all(CONTENT_TYPES.as_bytes())?;
        zip.start_file("_rels/.rels", options)?;
        zip.write_all(ROOT_RELS.as_bytes())?;
        zip.start_file("word/_rels/document.xml.rels", options)?;
        zip.write_all(DOCUMENT_RELS.as_bytes())?;
        zip.start_file("word/styles.xml", options)?;
        zip.write_all(styles.as_bytes())?;
        zip.start_file("word/document.xml", options)?;
        zip.write_all(document.as_bytes())?;
        zip.finish()?;
    }
    Ok(buffer.into_inner())
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

const DOCUMENT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;

fn markdown_to_body(markdown: &str) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end();
        let trimmed = line.trim_start();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // Pipe table: collect consecutive rows, skip the --- separator row
        if trimmed.starts_with('|') {
            let mut rows: Vec<Vec<String>> = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                let row = lines[i].trim().trim_matches('|');
                let cells: Vec<String> = row.split('|').map(|c| c.trim().to_string()).collect();
                let is_separator = cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')));
                if !is_separator {
                    rows.push(cells);
                }
                i += 1;
            }
            out.push_str(&table(&rows));
            continue;
        }

        if let Some((level, text)) = heading(trimmed) {
            let size = match level { 1 => 32, 2 => 28, _ => 28 };
            let center = level == 1;
            out.push_str(&paragraph(text, Some(size), true, center, None));
        } else if let Some(text) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            out.push_str(&paragraph(&format!("•\u{00A0}{}", text), None, false, false, Some(360)));
        } else if let Some(text) = numbered(trimmed) {
            out.push_str(&paragraph(text, None, false, false, Some(360)));
        } else if is_turn_heading(trimmed) {
            // A name line of the stenogram: room above, its paragraph close below (as in the PDF)
            out.push_str(&format!("<w:p><w:pPr><w:keepNext/><w:spacing w:before=\"280\" w:after=\"100\"/></w:pPr>{}</w:p>", runs(trimmed, None, false)));
        } else {
            out.push_str(&paragraph(trimmed, None, false, false, None));
        }
        i += 1;
    }
    out
}

/// «**Ім'я · 00:03**»: a whole line in bold, ending with a time
fn is_turn_heading(line: &str) -> bool {
    line.strip_prefix("**").and_then(|l| l.strip_suffix("**")).is_some_and(|l| !l.contains("**") && l.contains(" · ") && l.rsplit(' ').next().is_some_and(|t| t.contains(':') && t.chars().all(|c| c.is_ascii_digit() || c == ':')))
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&level) && line[level..].starts_with(' ') {
        Some((level, line[level..].trim()))
    } else {
        None
    }
}

/// "1. text" keeps its number as written
fn numbered(line: &str) -> Option<&str> {
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && line[digits..].starts_with(". ") { Some(line) } else { None }
}

fn paragraph(text: &str, size: Option<u32>, bold: bool, center: bool, indent: Option<u32>) -> String {
    let mut ppr = String::new();
    if center {
        ppr.push_str(r#"<w:jc w:val="center"/>"#);
    }
    if let Some(left) = indent {
        ppr.push_str(&format!(r#"<w:ind w:left="{left}"/>"#));
    }
    let ppr = if ppr.is_empty() { String::new() } else { format!("<w:pPr>{ppr}</w:pPr>") };
    format!("<w:p>{ppr}{}</w:p>", runs(text, size, bold))
}

/// Splits **bold** spans into separate runs
fn runs(text: &str, size: Option<u32>, bold_all: bool) -> String {
    let mut out = String::new();
    for (index, part) in text.split("**").enumerate() {
        if part.is_empty() {
            continue;
        }
        let bold = bold_all || index % 2 == 1;
        let mut rpr = String::new();
        if bold {
            rpr.push_str("<w:b/>");
        }
        if let Some(sz) = size {
            rpr.push_str(&format!(r#"<w:sz w:val="{sz}"/><w:szCs w:val="{sz}"/>"#));
        }
        let rpr = if rpr.is_empty() { String::new() } else { format!("<w:rPr>{rpr}</w:rPr>") };
        out.push_str(&format!(r#"<w:r>{rpr}<w:t xml:space="preserve">{}</w:t></w:r>"#, escape(part)));
    }
    out
}

fn table(rows: &[Vec<String>]) -> String {
    let border = r#"w:val="single" w:sz="4" w:space="0" w:color="000000""#;
    let mut out = format!(
        r#"<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="pct"/><w:tblBorders><w:top {border}/><w:left {border}/><w:bottom {border}/><w:right {border}/><w:insideH {border}/><w:insideV {border}/></w:tblBorders></w:tblPr>"#
    );
    for (index, row) in rows.iter().enumerate() {
        out.push_str("<w:tr>");
        for cell in row {
            out.push_str(&format!("<w:tc><w:p>{}</w:p></w:tc>", runs(cell, None, index == 0)));
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl><w:p/>");
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    /// PROTOKOL_STENO_MD: a markdown file; writes .docx and .pdf next to it
    #[test]
    #[ignore = "needs a document"]
    fn export_a_document() {
        let path = std::env::var("PROTOKOL_STENO_MD").unwrap();
        let md = std::fs::read_to_string(&path).unwrap();
        std::fs::write(format!("{path}.docx"), super::build_docx(&md).unwrap()).unwrap();
        std::fs::write(format!("{path}.pdf"), crate::ukrainian::pdf_export::build_pdf("Стенограма", &md).unwrap()).unwrap();
    }

    use super::*;

    #[test]
    fn stenogram_name_lines_are_spaced() {
        assert!(super::is_turn_heading("**Сокіл · 00:03**"));
        assert!(!super::is_turn_heading("**Дата:** 08.11.2026"));
        assert!(!super::is_turn_heading("**Важливо**"));
        assert!(super::markdown_to_body("**Сокіл · 00:03**\n\nТекст").contains("w:before=\"280\""));
    }

    #[test]
        fn builds_a_readable_docx() {
        let md = "# Підсумок наради\n\n**Коротко:** звіт\n\n- пункт\n\n| Завдання | Хто |\n|---|---|\n| Подати заявку | Петренко |\n";
        let bytes = build_docx(md).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("word/document.xml").unwrap(), &mut xml).unwrap();
        assert!(xml.contains("Підсумок наради"));
        assert!(xml.contains("<w:tbl>"));
        assert!(xml.contains("Подати заявку"));
        assert!(!xml.contains("---"));
    }

    #[test]
    fn escapes_xml() {
        assert_eq!(escape("a < b & \"c\""), "a &lt; b &amp; &quot;c&quot;");
    }
}
