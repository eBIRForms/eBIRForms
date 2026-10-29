//! Small, renderer-independent PDF helpers.
//!
//! These helpers support confirmation receipts and appending receipt text to a
//! validated HTML-generated form. They do not render tax forms or depend on a
//! separate document compiler.

use lopdf::{dictionary, Document, Object, Stream};

const A4_WIDTH_POINTS: u32 = 595;
const A4_HEIGHT_POINTS: u32 = 842;

#[derive(Debug, thiserror::Error)]
pub enum PdfUtilityError {
    #[error("invalid PDF: {0}")]
    InvalidPdf(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Build a simple A4 PDF from text lines for a submission confirmation receipt.
pub fn build_simple_confirmation_pdf(lines: &[String]) -> Vec<u8> {
    let wrapped_lines = wrap_lines(lines, 90);
    build_simple_pdf(A4_WIDTH_POINTS, A4_HEIGHT_POINTS, &wrapped_lines)
}

/// Append text pages to an existing form PDF while preserving its paper size.
pub fn append_text_pages_to_pdf(
    pdf_bytes: &[u8],
    lines: &[String],
) -> Result<Vec<u8>, PdfUtilityError> {
    let wrapped_lines = wrap_lines(lines, 90);
    let mut document = Document::load_mem(pdf_bytes)
        .map_err(|error| PdfUtilityError::InvalidPdf(error.to_string()))?;

    if lines.is_empty() {
        return save_document(&mut document);
    }

    let (width, height) = first_page_size(&document)?;
    let catalog = document
        .catalog()
        .map_err(|error| PdfUtilityError::InvalidPdf(format!("missing catalog: {error}")))?;
    let pages_reference = catalog
        .get(b"Pages")
        .and_then(Object::as_reference)
        .map_err(|error| {
            PdfUtilityError::InvalidPdf(format!("missing Pages reference: {error}"))
        })?;

    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let chunks: Vec<&[String]> = wrapped_lines.chunks(55).collect();
    let page_count = chunks.len();
    let mut new_page_ids = Vec::with_capacity(page_count);

    for (index, chunk) in chunks.iter().enumerate() {
        let mut content = String::from("BT\n/F1 10 Tf\n12 TL\n");
        content.push_str(&format!("40 {} Td\n", (height - 50.0).max(0.0)));
        for (line_index, line) in chunk.iter().enumerate() {
            if line_index > 0 {
                content.push_str("T*\n");
            }
            content.push_str(&format!("({}) Tj\n", escape_pdf_text(line)));
        }
        content.push_str("T*\n");
        content.push_str(&format!(
            "(Confirmation - Page {} of {}) Tj\nET",
            index + 1,
            page_count
        ));

        let content_id = document.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(pages_reference),
            "MediaBox" => vec![
                Object::Integer(0),
                Object::Integer(0),
                pdf_number(width),
                pdf_number(height),
            ],
            "CropBox" => vec![
                Object::Integer(0),
                Object::Integer(0),
                pdf_number(width),
                pdf_number(height),
            ],
            "Resources" => dictionary! {
                "Font" => dictionary! { "F1" => Object::Reference(font_id) },
            },
            "Contents" => Object::Reference(content_id),
        });
        new_page_ids.push(page_id);
    }

    let pages = document
        .get_object_mut(pages_reference)
        .and_then(Object::as_dict_mut)
        .map_err(|error| {
            PdfUtilityError::InvalidPdf(format!("invalid Pages dictionary: {error}"))
        })?;
    let kids = pages
        .get_mut(b"Kids")
        .and_then(Object::as_array_mut)
        .map_err(|error| {
            PdfUtilityError::InvalidPdf(format!("invalid Pages/Kids array: {error}"))
        })?;
    kids.extend(new_page_ids.iter().copied().map(Object::Reference));
    let count = pages
        .get_mut(b"Count")
        .map_err(|error| PdfUtilityError::InvalidPdf(format!("missing Pages/Count: {error}")))?;
    let Object::Integer(count) = count else {
        return Err(PdfUtilityError::InvalidPdf(
            "Pages/Count is not an integer".to_string(),
        ));
    };
    *count += i64::try_from(new_page_ids.len())
        .map_err(|_| PdfUtilityError::InvalidPdf("too many appended pages".to_string()))?;

    save_document(&mut document)
}

fn first_page_size(document: &Document) -> Result<(f64, f64), PdfUtilityError> {
    let (_, page_id) = document
        .get_pages()
        .into_iter()
        .min_by_key(|(number, _)| *number)
        .ok_or_else(|| PdfUtilityError::InvalidPdf("source PDF has no pages".to_string()))?;
    let media_box = inherited_page_value(document, page_id, b"MediaBox").ok_or_else(|| {
        PdfUtilityError::InvalidPdf("first page has no inherited MediaBox".to_string())
    })?;
    let media_box = resolved_object(document, media_box)
        .and_then(|object| object.as_array().ok())
        .ok_or_else(|| {
            PdfUtilityError::InvalidPdf("first page MediaBox is not an array".to_string())
        })?;
    if media_box.len() != 4 {
        return Err(PdfUtilityError::InvalidPdf(format!(
            "first page MediaBox must have four coordinates, found {}",
            media_box.len()
        )));
    }
    let mut coordinates = [0.0; 4];
    for (index, coordinate) in media_box.iter().enumerate() {
        coordinates[index] = object_number(document, coordinate).ok_or_else(|| {
            PdfUtilityError::InvalidPdf(format!(
                "first page MediaBox coordinate {index} is not a finite number"
            ))
        })?;
    }
    let width = coordinates[2] - coordinates[0];
    let height = coordinates[3] - coordinates[1];
    if !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || width > f64::from(f32::MAX)
        || height > f64::from(f32::MAX)
    {
        return Err(PdfUtilityError::InvalidPdf(
            "first page MediaBox must have finite positive dimensions representable in PDF"
                .to_string(),
        ));
    }
    Ok((width, height))
}

fn inherited_page_value<'a>(
    document: &'a Document,
    mut object_id: lopdf::ObjectId,
    key: &[u8],
) -> Option<&'a Object> {
    for _ in 0..64 {
        let dictionary = document.get_dictionary(object_id).ok()?;
        if let Ok(value) = dictionary.get(key) {
            return Some(value);
        }
        object_id = dictionary.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn resolved_object<'a>(document: &'a Document, mut value: &'a Object) -> Option<&'a Object> {
    for _ in 0..64 {
        match value {
            Object::Reference(object_id) => value = document.get_object(*object_id).ok()?,
            _ => return Some(value),
        }
    }
    None
}

fn object_number(document: &Document, value: &Object) -> Option<f64> {
    let value = resolved_object(document, value)?;
    let number = match value {
        Object::Integer(value) => *value as f64,
        Object::Real(value) => f64::from(*value),
        _ => return None,
    };
    number.is_finite().then_some(number)
}

fn pdf_number(value: f64) -> Object {
    if value.fract().abs() <= f64::EPSILON && value <= i64::MAX as f64 {
        Object::Integer(value as i64)
    } else {
        Object::Real(value as f32)
    }
}

fn save_document(document: &mut Document) -> Result<Vec<u8>, PdfUtilityError> {
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .map_err(|error| std::io::Error::other(format!("save PDF: {error}")))?;
    Ok(bytes)
}

fn build_simple_pdf(width: u32, height: u32, lines: &[String]) -> Vec<u8> {
    let pages: Vec<&[String]> = lines.chunks(42).collect();
    let page_count = pages.len().max(1);
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            (0..page_count)
                .map(|index| format!("{} 0 R", 4 + index))
                .collect::<Vec<_>>()
                .join(" "),
            page_count
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let first_content_id = 4 + page_count;
    for index in 0..page_count {
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            first_content_id + index
        ));
    }
    for (index, page_lines) in pages.iter().enumerate() {
        let stream = page_stream(height, page_lines, index + 1, page_count);
        objects.push(format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            stream.len(),
            stream
        ));
    }
    if pages.is_empty() {
        let stream = page_stream(height, &[], 1, 1);
        objects.push(format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            stream.len(),
            stream
        ));
    }

    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", index + 1, object));
    }
    let xref_offset = pdf.len();
    pdf.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
        objects.len() + 1,
        xref_offset
    ));
    pdf.into_bytes()
}

fn page_stream(height: u32, lines: &[String], page: usize, page_count: usize) -> String {
    let mut output = format!(
        "BT\n/F1 11 Tf\n14 TL\n50 {} Td\n",
        height.saturating_sub(60)
    );
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            output.push_str("T*\n");
        }
        output.push_str(&format!("({}) Tj\n", escape_pdf_text(line)));
    }
    output.push_str(&format!("T*\n(Page {page} of {page_count}) Tj\nET"));
    output
}

fn escape_pdf_text(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '(' => "\\(".to_string(),
            ')' => "\\)".to_string(),
            '\\' => "\\\\".to_string(),
            character if character.is_ascii() => character.to_string(),
            _ => " ".to_string(),
        })
        .collect()
}

fn wrap_lines(lines: &[String], max_characters: usize) -> Vec<String> {
    let mut wrapped = Vec::new();
    for line in lines {
        if line.is_empty() {
            wrapped.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in line.split_whitespace() {
            let separator = if current.is_empty() { "" } else { " " };
            if current.chars().count() + separator.len() + word.chars().count() <= max_characters {
                current.push_str(separator);
                current.push_str(word);
                continue;
            }
            if !current.is_empty() {
                wrapped.push(std::mem::take(&mut current));
            }
            let mut characters = word.chars().peekable();
            while characters.peek().is_some() {
                let chunk: String = characters.by_ref().take(max_characters).collect();
                if characters.peek().is_some() {
                    wrapped.push(chunk);
                } else {
                    current = chunk;
                }
            }
        }
        if !current.is_empty() {
            wrapped.push(current);
        }
    }
    wrapped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_page_id(document: &Document) -> lopdf::ObjectId {
        *document
            .get_pages()
            .values()
            .next()
            .expect("test PDF should have a page")
    }

    fn replace_first_page_media_box(document: &mut Document, media_box: impl Into<Object>) {
        let page_id = first_page_id(document);
        document
            .get_dictionary_mut(page_id)
            .expect("test page should be a dictionary")
            .set("MediaBox", media_box);
    }

    fn inherit_first_page_media_box(document: &mut Document) {
        let page_id = first_page_id(document);
        let parent_id = document
            .get_dictionary(page_id)
            .expect("test page should be a dictionary")
            .get(b"Parent")
            .expect("test page should have a parent")
            .as_reference()
            .expect("test page parent should be indirect");
        let media_box = document
            .get_dictionary_mut(page_id)
            .expect("test page should be a dictionary")
            .remove(b"MediaBox")
            .expect("test page should have a MediaBox");
        document
            .get_dictionary_mut(parent_id)
            .expect("test page parent should be a dictionary")
            .set("MediaBox", media_box);
    }

    fn save_test_document(document: &mut Document) -> Vec<u8> {
        save_document(document).expect("test PDF should save")
    }

    fn appended_page_box(pdf_bytes: &[u8], key: &[u8]) -> [f64; 4] {
        let document = Document::load_mem(pdf_bytes).expect("combined PDF should load");
        let page_id = *document
            .get_pages()
            .get(&2)
            .expect("combined PDF should have an appended page");
        let page = document
            .get_dictionary(page_id)
            .expect("appended page should be a dictionary");
        let values = page
            .get(key)
            .expect("appended page should have requested page box")
            .as_array()
            .expect("appended page box should be an array");
        std::array::from_fn(|index| {
            object_number(&document, &values[index])
                .expect("appended page box coordinate should be numeric")
        })
    }

    #[test]
    fn confirmation_pdf_is_valid_and_nonempty() {
        let bytes = build_simple_confirmation_pdf(&["BIR confirmation".to_string()]);
        let document = Document::load_mem(&bytes).expect("confirmation should be valid PDF");
        assert_eq!(document.get_pages().len(), 1);
        assert!(!document
            .get_page_content(*document.get_pages().values().next().unwrap())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn appended_pages_preserve_form_geometry() {
        let source = build_simple_pdf(612, 936, &["Form page".to_string()]);
        let combined = append_text_pages_to_pdf(&source, &["Confirmation".to_string()])
            .expect("append should succeed");
        assert_eq!(
            appended_page_box(&combined, b"MediaBox"),
            [0.0, 0.0, 612.0, 936.0]
        );
    }

    #[test]
    fn appended_pages_set_crop_box_to_form_geometry() {
        let source = build_simple_pdf(612, 936, &["Form page".to_string()]);
        let combined = append_text_pages_to_pdf(&source, &["Confirmation".to_string()])
            .expect("append should succeed");
        assert_eq!(
            appended_page_box(&combined, b"CropBox"),
            [0.0, 0.0, 612.0, 936.0]
        );
    }

    #[test]
    fn appended_pages_preserve_inherited_letter_geometry() {
        let source = build_simple_pdf(612, 792, &["Form page".to_string()]);
        let mut document = Document::load_mem(&source).expect("source PDF should load");
        inherit_first_page_media_box(&mut document);
        let source = save_test_document(&mut document);

        let combined = append_text_pages_to_pdf(&source, &["Confirmation".to_string()])
            .expect("inherited MediaBox should be resolved");
        assert_eq!(
            appended_page_box(&combined, b"MediaBox"),
            [0.0, 0.0, 612.0, 792.0]
        );
    }

    #[test]
    fn appended_pages_resolve_indirect_media_box() {
        let source = build_simple_pdf(612, 936, &["Form page".to_string()]);
        let mut document = Document::load_mem(&source).expect("source PDF should load");
        let media_box_id = document.add_object(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]);
        replace_first_page_media_box(&mut document, Object::Reference(media_box_id));
        let source = save_test_document(&mut document);

        let combined = append_text_pages_to_pdf(&source, &["Confirmation".to_string()])
            .expect("indirect MediaBox should be resolved");
        assert_eq!(
            appended_page_box(&combined, b"MediaBox"),
            [0.0, 0.0, 612.0, 792.0]
        );
    }

    #[test]
    fn appended_pages_preserve_legal_geometry() {
        let source = build_simple_pdf(612, 1008, &["Form page".to_string()]);
        let combined = append_text_pages_to_pdf(&source, &["Confirmation".to_string()])
            .expect("append should succeed");
        assert_eq!(
            appended_page_box(&combined, b"MediaBox"),
            [0.0, 0.0, 612.0, 1008.0]
        );
    }

    #[test]
    fn appended_pages_use_dimensions_from_nonzero_media_box_origin() {
        let source = build_simple_pdf(612, 936, &["Form page".to_string()]);
        let mut document = Document::load_mem(&source).expect("source PDF should load");
        replace_first_page_media_box(
            &mut document,
            vec![10.into(), 20.into(), 622.into(), 956.into()],
        );
        let source = save_test_document(&mut document);

        let combined = append_text_pages_to_pdf(&source, &["Confirmation".to_string()])
            .expect("valid nonzero MediaBox origin should be supported");
        assert_eq!(
            appended_page_box(&combined, b"MediaBox"),
            [0.0, 0.0, 612.0, 936.0]
        );
    }

    #[test]
    fn missing_media_box_is_rejected() {
        let source = build_simple_pdf(612, 936, &["Form page".to_string()]);
        let mut document = Document::load_mem(&source).expect("source PDF should load");
        let page_id = first_page_id(&document);
        document
            .get_dictionary_mut(page_id)
            .expect("test page should be a dictionary")
            .remove(b"MediaBox");
        let source = save_test_document(&mut document);

        let error = append_text_pages_to_pdf(&source, &["Confirmation".to_string()])
            .expect_err("missing MediaBox must fail closed");
        assert!(matches!(error, PdfUtilityError::InvalidPdf(_)));
    }

    #[test]
    fn invalid_media_box_is_rejected() {
        let source = build_simple_pdf(612, 936, &["Form page".to_string()]);
        let mut document = Document::load_mem(&source).expect("source PDF should load");
        replace_first_page_media_box(
            &mut document,
            vec![0.into(), 0.into(), 0.into(), 936.into()],
        );
        let source = save_test_document(&mut document);

        let error = append_text_pages_to_pdf(&source, &["Confirmation".to_string()])
            .expect_err("non-positive MediaBox width must fail closed");
        assert!(matches!(error, PdfUtilityError::InvalidPdf(_)));
    }

    #[test]
    fn invalid_source_is_rejected() {
        let error = append_text_pages_to_pdf(b"not a PDF", &["Receipt".to_string()])
            .expect_err("invalid PDF must fail");
        assert!(matches!(error, PdfUtilityError::InvalidPdf(_)));
    }
}
