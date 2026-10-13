//! Direct DOCX pages and XLSX sheet regions, without an intermediate PDF.

use cairo::{Context, Format, ImageSurface};
use gio::prelude::*;
use gridcraft_core::{CellRef, RangeRef, Value, col_to_letters};
use gridcraft_model::{HAlign, PatternType, Style, VAlign, Visibility, Workbook};
use gridcraft_numfmt::{NumberFormat, format_value};
use pango::{FontDescription, Weight, WrapMode};

use crate::PreviewLimits;

#[derive(Debug)]
pub(crate) enum NativeDocument {
    Word {
        document: Box<wordcraft_doc::Document>,
        layout: wordcraft_layout::DocLayout,
    },
    Sheets {
        workbook: Box<Workbook>,
        pages: Vec<SheetPage>,
    },
}

#[derive(Debug)]
pub(crate) struct SheetPage {
    sheet: usize,
    range: RangeRef,
}

fn check_cancelled(cancellable: &gio::Cancellable) -> Result<(), String> {
    if cancellable.is_cancelled() {
        Err("Document preview cancelled".to_owned())
    } else {
        Ok(())
    }
}

impl NativeDocument {
    pub(crate) fn read(
        bytes: &[u8],
        extension: &str,
        cancellable: &gio::Cancellable,
    ) -> Result<Self, String> {
        check_cancelled(cancellable)?;
        let document = match extension {
            "docx" => {
                let document = wordcraft_docx::read(bytes).map_err(|error| error.to_string())?;
                check_cancelled(cancellable)?;
                let layout = wordcraft_layout::layout(
                    &document,
                    &mut wordcraft_layout::LayoutCache::new(),
                    &wordcraft_layout::LayoutOptions {
                        hide_deleted: true,
                        ..Default::default()
                    },
                );
                if layout.pages.is_empty() || layout.pages.len() > 1000 {
                    return Err("Native preview is limited to 1000 pages".to_owned());
                }
                Self::Word {
                    document: Box::new(document),
                    layout,
                }
            }
            "xlsx" => {
                let (workbook, report) =
                    gridcraft_xlsx::read_xlsx(bytes).map_err(|error| error.to_string())?;
                // Keep the faithful conversion fallback for content this renderer
                // cannot yet draw, rather than silently hiding charts or pictures.
                if !report.warnings.is_empty() {
                    return Err(format!(
                        "Spreadsheet import: {}",
                        report.warnings.join("; ")
                    ));
                }
                let mut pages = Vec::new();
                for (sheet_index, sheet) in workbook.sheets.iter().enumerate() {
                    check_cancelled(cancellable)?;
                    if sheet.visibility != Visibility::Visible {
                        continue;
                    }
                    if !sheet.charts.is_empty()
                        || !sheet.images.is_empty()
                        || !sheet.shapes.is_empty()
                        || !sheet.cell_pictures.is_empty()
                        || !sheet.sparklines.is_empty()
                        || !sheet.cond_formats.is_empty()
                        || sheet.right_to_left
                    {
                        return Err("This spreadsheet needs the full-layout renderer".to_owned());
                    }
                    if sheet.cells.iter().any(|(address, _)| {
                        workbook.styles.get(sheet.style_id(address)).align.rotation != 0
                    }) {
                        return Err(
                            "Rotated spreadsheet text needs the full-layout renderer".to_owned()
                        );
                    }
                    let mut used = sheet
                        .used_range()
                        .unwrap_or_else(|| RangeRef::cell(CellRef::new(0, 0)));
                    for merged in &sheet.merges {
                        used = used.union(merged);
                    }
                    if used.height() > 100_000 || used.width() > 10_000 {
                        return Err("Spreadsheet range is too large for native preview".to_owned());
                    }
                    let rows = axis_chunks(
                        used.start.row,
                        used.end.row,
                        900.0,
                        |row| sheet.row_height(row),
                        &sheet.merges,
                        true,
                    )?;
                    let cols = axis_chunks(
                        used.start.col,
                        used.end.col,
                        750.0,
                        |col| sheet.col_width(col),
                        &sheet.merges,
                        false,
                    )?;
                    for &(r0, r1) in &rows {
                        for &(c0, c1) in &cols {
                            if pages.len() >= 1000 {
                                return Err(
                                    "Native preview is limited to 1000 sheet regions".to_owned()
                                );
                            }
                            pages.push(SheetPage {
                                sheet: sheet_index,
                                range: RangeRef::new(CellRef::new(r0, c0), CellRef::new(r1, c1)),
                            });
                        }
                    }
                }
                if pages.is_empty() {
                    return Err("Workbook has no visible sheets".to_owned());
                }
                Self::Sheets {
                    workbook: Box::new(workbook),
                    pages,
                }
            }
            _ => return Err("No direct renderer for this format".to_owned()),
        };
        check_cancelled(cancellable)?;
        Ok(document)
    }

    pub(crate) fn page_count(&self) -> u32 {
        match self {
            Self::Word { layout, .. } => layout.pages.len() as u32,
            Self::Sheets { pages, .. } => pages.len() as u32,
        }
    }

    pub(crate) fn page_label(&self, number: u32) -> Option<String> {
        match self {
            Self::Sheets { workbook, pages } => pages
                .get(number.checked_sub(1)? as usize)
                .map(|page| format!("{} · {}", workbook.sheets[page.sheet].name, page.range)),
            _ => None,
        }
    }

    pub(crate) fn render(
        &self,
        number: u32,
        limits: PreviewLimits,
        cancellable: &gio::Cancellable,
    ) -> Result<Vec<u8>, String> {
        check_cancelled(cancellable)?;
        let index = number.checked_sub(1).ok_or("Invalid page number")? as usize;
        let bytes = match self {
            Self::Word { document, layout } => {
                let page = layout
                    .pages
                    .get(index)
                    .ok_or("Page is outside the document")?;
                let scale = (limits.image_width as f32 / page.w)
                    .min(limits.image_height as f32 / page.h)
                    .clamp(0.01, 4.0);
                let image = wordcraft_render::render_page(
                    document,
                    page,
                    scale,
                    &wordcraft_render::RenderOptions::default(),
                );
                image.to_png()
            }
            Self::Sheets { workbook, pages } => {
                let page = pages.get(index).ok_or("Page is outside the workbook")?;
                render_sheet(workbook, page, limits, cancellable)?
            }
        };
        check_cancelled(cancellable)?;
        if bytes.is_empty() {
            return Err("Document renderer returned an empty page".to_owned());
        }
        Ok(bytes)
    }
}

fn axis_chunks(
    start: u32,
    end: u32,
    maximum: f32,
    size: impl Fn(u32) -> f32,
    merges: &[RangeRef],
    rows: bool,
) -> Result<Vec<(u32, u32)>, String> {
    let mut chunks = Vec::new();
    let mut first = start;
    while first <= end {
        let mut last = first;
        let mut total = 0.0;
        while last <= end {
            let length = size(last);
            if !length.is_finite() || length < 0.0 || length > 10_000.0 {
                return Err("Invalid spreadsheet dimensions".to_owned());
            }
            if total + length > maximum && last > first {
                break;
            }
            total += length;
            last += 1;
        }
        last = last.saturating_sub(1).max(first).min(end);
        // Extend a region until no merged cell crosses its boundary.
        loop {
            let expanded = merges.iter().fold(last, |limit, merge| {
                let (a, b) = if rows {
                    (merge.start.row, merge.end.row)
                } else {
                    (merge.start.col, merge.end.col)
                };
                if a <= limit && b > limit {
                    limit.max(b.min(end))
                } else {
                    limit
                }
            });
            if expanded == last {
                break;
            }
            last = expanded;
        }
        if last - first > 10_000 {
            return Err("Merged spreadsheet region is too large".to_owned());
        }
        chunks.push((first, last));
        if chunks.len() > 1000 {
            return Err("Too many spreadsheet regions".to_owned());
        }
        first = last + 1;
    }
    Ok(chunks)
}

fn color(context: &Context, rgb: [u8; 3]) {
    context.set_source_rgb(
        f64::from(rgb[0]) / 255.0,
        f64::from(rgb[1]) / 255.0,
        f64::from(rgb[2]) / 255.0,
    );
}

fn line_positions(start: u32, end: u32, size: impl Fn(u32) -> f32, offset: f64) -> Vec<f64> {
    let mut positions = vec![offset];
    for index in start..=end {
        positions.push(positions.last().copied().unwrap_or(offset) + f64::from(size(index)));
    }
    positions
}

fn render_sheet(
    workbook: &Workbook,
    page: &SheetPage,
    limits: PreviewLimits,
    cancellable: &gio::Cancellable,
) -> Result<Vec<u8>, String> {
    let sheet = &workbook.sheets[page.sheet];
    let range = page.range;
    let xs = line_positions(
        range.start.col,
        range.end.col,
        |col| sheet.col_width(col),
        52.0,
    );
    let ys = line_positions(
        range.start.row,
        range.end.row,
        |row| sheet.row_height(row),
        60.0,
    );
    let width = xs.last().copied().unwrap_or(52.0) + 12.0;
    let height = ys.last().copied().unwrap_or(60.0) + 12.0;
    let scale = (f64::from(limits.image_width) / width)
        .min(f64::from(limits.image_height) / height)
        .clamp(0.01, 2.0);
    let surface = ImageSurface::create(
        Format::ARgb32,
        (width * scale).ceil().max(1.0) as i32,
        (height * scale).ceil().max(1.0) as i32,
    )
    .map_err(|error| error.to_string())?;
    let context = Context::new(&surface).map_err(|error| error.to_string())?;
    context.scale(scale, scale);
    context.set_source_rgb(1.0, 1.0, 1.0);
    context.paint().map_err(|error| error.to_string())?;
    let layout = pangocairo::functions::create_layout(&context);
    let header = Style::default();
    draw_text(
        &context,
        &layout,
        &sheet.name,
        &header,
        [0, 0, 0],
        (12.0, 8.0, width - 24.0, 24.0),
        false,
    )?;
    context.set_source_rgb(0.93, 0.94, 0.96);
    context.rectangle(12.0, 38.0, width - 24.0, 22.0);
    context.rectangle(12.0, 60.0, 40.0, height - 72.0);
    context.fill().map_err(|error| error.to_string())?;
    for col in range.start.col..=range.end.col {
        let i = (col - range.start.col) as usize;
        draw_text(
            &context,
            &layout,
            &col_to_letters(col),
            &header,
            [70, 70, 70],
            (xs[i], 38.0, xs[i + 1] - xs[i], 22.0),
            false,
        )?;
    }
    for row in range.start.row..=range.end.row {
        check_cancelled(cancellable)?;
        let r = (row - range.start.row) as usize;
        if ys[r + 1] <= ys[r] {
            continue;
        }
        draw_text(
            &context,
            &layout,
            &(row + 1).to_string(),
            &header,
            [70, 70, 70],
            (12.0, ys[r], 40.0, ys[r + 1] - ys[r]),
            true,
        )?;
        for col in range.start.col..=range.end.col {
            let c = (col - range.start.col) as usize;
            if xs[c + 1] <= xs[c] {
                continue;
            }
            let address = CellRef::new(row, col);
            let merged = sheet.merges.iter().find(|merged| merged.contains(address));
            if merged.is_some_and(|merged| merged.start != address) {
                continue;
            }
            let end = merged.map_or(address, |merged| merged.end);
            let right = xs[(end.col.min(range.end.col) - range.start.col + 1) as usize];
            let bottom = ys[(end.row.min(range.end.row) - range.start.row + 1) as usize];
            let rect = (xs[c], ys[r], right - xs[c], bottom - ys[r]);
            let cell = sheet.cell(address);
            let table_style = sheet.table_at(address).map(|table| {
                gridcraft_engine::tables::table_cell_style(workbook, page.sheet, table, address)
            });
            let style = table_style
                .as_ref()
                .unwrap_or_else(|| workbook.styles.get(sheet.style_id(address)));
            if style.fill.pattern != PatternType::None {
                color(
                    &context,
                    style
                        .fill
                        .fg
                        .resolve(&workbook.theme)
                        .unwrap_or([255, 255, 255]),
                );
                context.rectangle(rect.0, rect.1, rect.2, rect.3);
                context.fill().map_err(|error| error.to_string())?;
            }
            if sheet.show_gridlines {
                color(&context, [210, 214, 220]);
                context.set_line_width(0.5);
                context.rectangle(rect.0, rect.1, rect.2, rect.3);
                context.stroke().map_err(|error| error.to_string())?;
            }
            for (border, x0, y0, x1, y1) in [
                (style.borders.left, rect.0, rect.1, rect.0, bottom),
                (style.borders.right, right, rect.1, right, bottom),
                (style.borders.top, rect.0, rect.1, right, rect.1),
                (style.borders.bottom, rect.0, bottom, right, bottom),
            ] {
                if border.is_none() {
                    continue;
                }
                color(
                    &context,
                    border.color.resolve(&workbook.theme).unwrap_or([0, 0, 0]),
                );
                context.set_line_width(f64::from(border.style.width()));
                context.move_to(x0, y0);
                context.line_to(x1, y1);
                context.stroke().map_err(|error| error.to_string())?;
            }
            if let Some(cell) = cell {
                let formatted = format_value(
                    &cell.value,
                    &NumberFormat::parse(style.num_fmt.as_str()),
                    workbook.date_system,
                );
                let text = if sheet.show_formulas {
                    cell.input_text()
                } else if cell.value == Value::Empty && cell.formula.is_some() {
                    // Uncalculated formulas must not silently look like empty cells.
                    cell.input_text()
                } else {
                    formatted.text
                };
                draw_text(
                    &context,
                    &layout,
                    &text,
                    style,
                    formatted.color.map(format_color).unwrap_or_else(|| {
                        style
                            .font
                            .color
                            .resolve(&workbook.theme)
                            .unwrap_or([0, 0, 0])
                    }),
                    rect,
                    formatted.numeric,
                )?;
            }
        }
    }
    let mut bytes = Vec::new();
    surface
        .write_to_png(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn format_color(color: gridcraft_numfmt::FormatColor) -> [u8; 3] {
    use gridcraft_numfmt::FormatColor;
    match color {
        FormatColor::Red => [255, 0, 0],
        FormatColor::Green => [0, 128, 0],
        FormatColor::Blue => [0, 0, 255],
        FormatColor::White => [255, 255, 255],
        FormatColor::Cyan => [0, 255, 255],
        FormatColor::Magenta => [255, 0, 255],
        FormatColor::Yellow => [255, 255, 0],
        _ => [0, 0, 0],
    }
}

fn draw_text(
    context: &Context,
    layout: &pango::Layout,
    text: &str,
    style: &Style,
    rgb: [u8; 3],
    rect: (f64, f64, f64, f64),
    numeric: bool,
) -> Result<(), String> {
    let (x, y, width, height) = rect;
    if width <= 0.0 || height <= 0.0 {
        return Ok(());
    }
    context.save().map_err(|error| error.to_string())?;
    context.rectangle(x, y, width, height);
    context.clip();
    let mut font = FontDescription::new();
    font.set_family(&style.font.name);
    font.set_absolute_size(f64::from(style.font.size.clamp(1.0, 96.0)) * f64::from(pango::SCALE));
    font.set_weight(if style.font.bold {
        Weight::Bold
    } else {
        Weight::Normal
    });
    font.set_style(if style.font.italic {
        pango::Style::Italic
    } else {
        pango::Style::Normal
    });
    layout.set_font_description(Some(&font));
    let attributes = pango::AttrList::new();
    let underline = match style.font.underline {
        gridcraft_model::Underline::Double | gridcraft_model::Underline::DoubleAccounting => {
            pango::Underline::Double
        }
        gridcraft_model::Underline::None => pango::Underline::None,
        _ => pango::Underline::Single,
    };
    attributes.insert(pango::AttrInt::new_underline(underline));
    attributes.insert(pango::AttrInt::new_strikethrough(style.font.strike));
    layout.set_attributes(Some(&attributes));
    layout.set_text(text);
    layout.set_width(if style.align.wrap {
        ((width - 6.0).max(1.0) * f64::from(pango::SCALE)) as i32
    } else {
        -1
    });
    layout.set_wrap(WrapMode::WordChar);
    let (text_width, text_height) = layout.pixel_size();
    let dx = match style.align.h {
        HAlign::Right => width - f64::from(text_width) - 3.0,
        HAlign::Center | HAlign::CenterAcross => (width - f64::from(text_width)) / 2.0,
        HAlign::General if numeric => width - f64::from(text_width) - 3.0,
        _ => 3.0 + f64::from(style.align.indent) * 8.0,
    };
    let dy = match style.align.v {
        VAlign::Top => 2.0,
        VAlign::Center => (height - f64::from(text_height)) / 2.0,
        _ => height - f64::from(text_height) - 2.0,
    };
    color(context, rgb);
    context.move_to(x + dx.max(2.0), y + dy.max(1.0));
    pangocairo::functions::show_layout(context, layout);
    context.restore().map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;

    pub(crate) fn docx_fixture() -> Vec<u8> {
        let mut document = wordcraft_doc::Document::from_text(
            "Document preview\nПрямой просмотр DOCX — без LibreOffice\nFormatting and a table",
        );
        let mut table = wordcraft_doc::table::Table::new(3, 2, 450.0);
        for (row, values) in [
            ["Item", "Amount"],
            ["Revenue", "1234.50"],
            ["Расходы", "250.00"],
        ]
        .iter()
        .enumerate()
        {
            for (col, value) in values.iter().enumerate() {
                table.rows[row].cells[col] = wordcraft_doc::table::Cell::with_text(value);
            }
        }
        document
            .body
            .push(Arc::new(wordcraft_doc::Block::Table(table)));
        wordcraft_docx::write(&document).unwrap()
    }

    pub(crate) fn xlsx_fixture() -> Vec<u8> {
        let mut workbook = Workbook::new();
        let mut header = Style::default();
        header.font.bold = true;
        header.font.color = gridcraft_model::Color::rgb(255, 255, 255);
        header.fill = gridcraft_model::Fill::solid(gridcraft_model::Color::rgb(25, 70, 130));
        let header_id = workbook.styles.intern(header);
        let money = Style {
            num_fmt: gridcraft_model::NumFmt::new("$#,##0.00"),
            ..Default::default()
        };
        let money_id = workbook.styles.intern(money);
        let sheet = Arc::make_mut(&mut workbook.sheets[0]);
        sheet.name = "Budget".to_owned();
        for row in 0..70 {
            for col in 0..20 {
                sheet.set_value(
                    CellRef::new(row, col),
                    Value::Number(f64::from(row * 100 + col)),
                );
            }
        }
        sheet.set_value(CellRef::new(0, 0), Value::Text("Quarterly budget".into()));
        sheet
            .merges
            .push(RangeRef::new(CellRef::new(0, 0), CellRef::new(0, 2)));
        sheet.set_style(CellRef::new(0, 0), header_id);
        sheet.set_value(CellRef::new(1, 0), Value::Text("Revenue".into()));
        sheet.set_value(CellRef::new(1, 1), Value::Number(1234.5));
        sheet.set_style(CellRef::new(1, 1), money_id);
        let mut totals = gridcraft_model::Sheet::new("Totals");
        totals.set_value(CellRef::new(0, 0), Value::Text("Total".into()));
        workbook.sheets.push(Arc::new(totals));
        let mut hidden = gridcraft_model::Sheet::new("Hidden");
        hidden.visibility = Visibility::Hidden;
        workbook.sheets.push(Arc::new(hidden));
        gridcraft_xlsx::write_xlsx(&workbook).unwrap()
    }

    #[test]
    fn wordcraft_reads_and_renders_docx_text_and_tables_directly() {
        let cancellable = gio::Cancellable::new();
        let started = std::time::Instant::now();
        let document = NativeDocument::read(&docx_fixture(), "docx", &cancellable).unwrap();
        assert!(document.page_count() > 0);
        let NativeDocument::Word {
            document: source,
            layout,
        } = &document
        else {
            panic!("expected WordCraft document");
        };
        assert!(
            source
                .body
                .iter()
                .any(|block| matches!(block.as_ref(), wordcraft_doc::Block::Table(_)))
        );
        assert!(!layout.pages[0].items.is_empty());
        let image = document
            .render(1, PreviewLimits::default(), &cancellable)
            .unwrap();
        assert_visible_png(&image);
        save_visual_artifact("native-docx.png", &image);
        eprintln!(
            "Direct DOCX parse + layout + first page: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn gridcraft_preserves_cells_styles_merges_and_visible_sheets() {
        let cancellable = gio::Cancellable::new();
        let started = std::time::Instant::now();
        let document = NativeDocument::read(&xlsx_fixture(), "xlsx", &cancellable).unwrap();
        let NativeDocument::Sheets { workbook, pages } = &document else {
            panic!("expected GridCraft workbook");
        };
        assert_eq!(workbook.sheets[0].merges.len(), 1);
        assert_eq!(
            workbook.sheets[0].cell(CellRef::new(1, 1)).unwrap().value,
            Value::Number(1234.5)
        );
        assert_eq!(
            format_value(
                &Value::Number(1234.5),
                &NumberFormat::parse("$#,##0.00"),
                workbook.date_system
            )
            .text,
            "$1,234.50"
        );
        assert!(pages.len() > 2);
        assert!(pages.iter().all(|page| page.sheet != 2));
        assert!(document.page_label(1).unwrap().starts_with("Budget"));
        assert!(
            document
                .page_label(document.page_count())
                .unwrap()
                .starts_with("Totals")
        );
        let image = document
            .render(1, PreviewLimits::default(), &cancellable)
            .unwrap();
        assert_visible_png(&image);
        save_visual_artifact("native-xlsx.png", &image);
        assert_visible_png(
            &document
                .render(
                    document.page_count(),
                    PreviewLimits::default(),
                    &cancellable,
                )
                .unwrap(),
        );
        eprintln!(
            "Direct XLSX parse + two sheet regions: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn unsupported_spreadsheet_layout_uses_the_compatibility_path() {
        let (mut workbook, _) = gridcraft_xlsx::read_xlsx(&xlsx_fixture()).unwrap();
        let mut style = Style::default();
        style.align.rotation = 45;
        let rotated = workbook.styles.intern(style);
        Arc::make_mut(&mut workbook.sheets[0]).set_style(CellRef::new(1, 0), rotated);
        let bytes = gridcraft_xlsx::write_xlsx(&workbook).unwrap();
        assert!(
            NativeDocument::read(&bytes, "xlsx", &gio::Cancellable::new())
                .unwrap_err()
                .contains("Rotated")
        );
    }

    #[test]
    fn region_boundaries_never_split_merged_cells() {
        let merged = RangeRef::new(CellRef::new(0, 2), CellRef::new(0, 5));
        let chunks = axis_chunks(0, 9, 30.0, |_| 10.0, &[merged], false).unwrap();
        assert_eq!(chunks, [(0, 5), (6, 8), (9, 9)]);
        assert!(axis_chunks(0, 2, 30.0, |_| f32::NAN, &[], false).is_err());
    }

    #[test]
    fn native_loading_handles_cancellation_and_invalid_packages() {
        let cancellable = gio::Cancellable::new();
        assert!(NativeDocument::read(b"invalid", "docx", &cancellable).is_err());
        assert!(NativeDocument::read(b"invalid", "xlsx", &cancellable).is_err());
        cancellable.cancel();
        assert!(
            NativeDocument::read(&docx_fixture(), "docx", &cancellable)
                .unwrap_err()
                .contains("cancelled")
        );
    }

    fn save_visual_artifact(name: &str, bytes: &[u8]) {
        if let Some(directory) = std::env::var_os("PATHPILOT_TEST_PREVIEW_DIR") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(name), bytes).unwrap();
        }
    }

    fn assert_visible_png(bytes: &[u8]) {
        let mut surface = ImageSurface::create_from_png(&mut std::io::Cursor::new(bytes)).unwrap();
        assert!(surface.width() <= 1200 && surface.height() <= 1200);
        assert!(surface.width() > 0 && surface.height() > 0);
        let data = surface.data().unwrap();
        assert!(
            data.as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[0] < 200 && pixel[1] < 200 && pixel[2] < 200),
            "rendered page must contain ink"
        );
    }
}
