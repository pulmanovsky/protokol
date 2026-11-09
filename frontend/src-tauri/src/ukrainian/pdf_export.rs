//! PDF export of the meeting document, made on this Mac without network.
//!
//! Layout: the header line, then each paragraph; speaker lines
//! («**Ім'я** (05:50):») start with the name in bold; pages are numbered.
//! Text uses Arial from macOS (Cyrillic included), read at run time and not
//! bundled with the app. PDF writing: genpdf (Apache-2.0 OR MIT).

use std::path::PathBuf;

use genpdf::{elements, fonts, style, Element};
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;

const FONT_DIR: &str = "/System/Library/Fonts/Supplemental";

fn font_family() -> Result<fonts::FontFamily<fonts::FontData>, String> {
    let load = |name: &str| -> Result<fonts::FontData, String> {
        let bytes = std::fs::read(format!("{FONT_DIR}/{name}")).map_err(|e| format!("Шрифт {name}: {e}"))?;
        fonts::FontData::new(bytes, None).map_err(|e| e.to_string())
    };
    let regular = load("Arial.ttf")?;
    let bold = load("Arial Bold.ttf")?;
    Ok(fonts::FontFamily { regular: regular.clone(), bold: bold.clone(), italic: regular, bold_italic: bold })
}

/// One paragraph: «**bold**» spans in bold, other Markdown marks removed
fn paragraph(line: &str) -> elements::Paragraph {
    let mut p = elements::Paragraph::default();
    let mut bold = false;
    for (i, part) in line.split("**").enumerate() {
        if i > 0 {
            bold = !bold;
        }
        let text = part.replace("__", "").replace('\\', "");
        if text.is_empty() {
            continue;
        }
        if bold {
            p.push_styled(text, style::Effect::Bold);
        } else {
            p.push(text);
        }
    }
    p
}

/// Builds the PDF from the document's Markdown
pub fn build_pdf(title: &str, markdown: &str) -> Result<Vec<u8>, String> {
    let mut doc = genpdf::Document::new(font_family()?);
    doc.set_title(title);
    doc.set_font_size(11);
    doc.set_line_spacing(1.25);
    let mut decorator = genpdf::SimplePageDecorator::new();
    decorator.set_margins(18);
    decorator.set_header(|page| {
        elements::Paragraph::new(format!("{page}"))
            .aligned(genpdf::Alignment::Right)
            .styled(style::Style::new().with_font_size(9))
    });
    doc.set_page_decorator(decorator);

    let mut first = true;
    for block in markdown.split("\n\n") {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        for (i, raw) in block.lines().enumerate() {
            let line = raw.trim().trim_start_matches('#').trim();
            if line.is_empty() || line.chars().all(|c| c == '-' || c == '|' || c == ':' || c == ' ') {
                continue;
            }
            let para = paragraph(line);
            if first {
                doc.push(para.styled(style::Style::new().bold().with_font_size(14)));
                doc.push(elements::Break::new(1));
                first = false;
            } else if i == 0 {
                doc.push(para);
            } else {
                doc.push(para.padded(genpdf::Margins::trbl(0.5, 0, 0, 0)));
            }
        }
        doc.push(elements::Break::new(0.8));
    }
    let mut bytes = Vec::new();
    doc.render(&mut bytes).map_err(|e| format!("Не вдалося створити PDF: {e}"))?;
    Ok(bytes)
}

/// Asks where to save and writes the PDF
#[tauri::command]
pub async fn api_export_protocol_pdf<R: Runtime>(app: AppHandle<R>, title: String, markdown: String) -> Result<Option<String>, String> {
    let markdown = crate::summary::materials::without_source_marks(&markdown);
    let file_name = format!("{}.pdf", super::docx_export::sanitize_file_name(&title));
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_file_name(&file_name).add_filter("PDF", &["pdf"]).blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(picked) = picked else { return Ok(None) };
    let mut path: PathBuf = picked.into_path().map_err(|e| e.to_string())?;
    if path.extension().map_or(true, |ext| ext != "pdf") {
        path.set_extension("pdf");
    }
    let bytes = tauri::async_runtime::spawn_blocking(move || build_pdf(&title, &markdown))
        .await
        .map_err(|e| e.to_string())??;
    std::fs::write(&path, bytes).map_err(|e| format!("Не вдалося зберегти файл: {e}"))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_pdf_with_ukrainian_text() {
        if !std::path::Path::new(&format!("{FONT_DIR}/Arial.ttf")).exists() {
            return; // not macOS
        }
        let md = "Нарада 05.11.2026, 37 хв\n\n**Олена Петренко** (00:00):\nДобрий день, починаємо нараду.\n\n**Доповідач 2** (01:20):\nЗв'язок працює.\n";
        let pdf = build_pdf("Тест", md).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert!(pdf.len() > 1000);
        // Manual check: render a real document to a file
        if let (Ok(src), Ok(out)) = (std::env::var("PROTOKOL_PDF_SRC"), std::env::var("PROTOKOL_PDF_OUT")) {
            let md = std::fs::read_to_string(src).unwrap();
            std::fs::write(out, build_pdf("Стенограма", &md).unwrap()).unwrap();
        }
    }
}
