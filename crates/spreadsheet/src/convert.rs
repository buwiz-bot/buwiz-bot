//! The formats the sheet opens besides `.xlsx`: CSV and TSV (one sheet,
//! saved back as text) and legacy `.xls` (read by office_oxide, saved as
//! `.xlsx`). Pure functions over bytes and the BetterOffice model.

use betteroffice_xlsx::{
    CalculationOptions, Cell, CellInput, CellRef, CellValue, Sheet, SheetId, Workbook, WorkbookModel,
};

/// RFC 4180 rows: quoted fields may hold the separator, quotes (doubled)
/// and line breaks; `\r\n` and `\n` both end a record.
pub(crate) fn parse_delimited(text: &str, sep: char) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    // a field that started with a quote, so `""` is a value, not nothing
    let mut was_quoted = false;
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !was_quoted => {
                quoted = true;
                was_quoted = true;
            }
            c if c == sep => {
                row.push(std::mem::take(&mut field));
                was_quoted = false;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                was_quoted = false;
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || was_quoted || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// A delimited field as a cell value: plain decimal numbers become
/// numbers; anything else (leading zeros, `1,234`, dates, `TRUE`) stays
/// the text it was, so saving writes back what was read.
pub(crate) fn field_value(s: &str) -> CellValue {
    if s.is_empty() {
        return CellValue::Empty;
    }
    let plain = s.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'e' | b'E' | b'+'))
        && s.bytes().next().is_some_and(|b| b.is_ascii_digit() || b == b'-' || b == b'.');
    let digits = s.strip_prefix('-').unwrap_or(s);
    let leading_zero = digits.len() > 1 && digits.starts_with('0') && !digits.starts_with("0.");
    match s.parse::<f64>() {
        Ok(v) if plain && !leading_zero && v.is_finite() && number_text(v) == s => CellValue::Number { value: v },
        _ => CellValue::Text { value: s.to_owned() },
    }
}

/// A number as a delimited file writes it: the shortest text that reads
/// back as the same value.
pub(crate) fn number_text(v: f64) -> String {
    if v == 0.0 {
        return "0".to_owned();
    }
    let s = format!("{v}");
    // `{}` never uses an exponent: very large or small values do
    if s.len() > 21 {
        let e = format!("{v:e}");
        if e.parse::<f64>() == Ok(v) {
            return e;
        }
    }
    s
}

/// One sheet named `name` from delimited rows.
pub(crate) fn delimited_model(rows: &[Vec<String>], name: &str) -> WorkbookModel {
    let mut sheet = Sheet::new(name);
    for (r, row) in rows.iter().enumerate() {
        for (c, field) in row.iter().enumerate() {
            let value = field_value(field);
            if value != CellValue::Empty {
                sheet.set_cell(CellRef::new(r as u32, c as u32), Cell { value, ..Cell::default() });
            }
        }
    }
    WorkbookModel { sheets: vec![sheet], ..WorkbookModel::default() }
}

/// The text of `value` in a delimited file: a formula writes its result.
fn value_text(value: &CellValue) -> String {
    match value {
        CellValue::Empty => String::new(),
        CellValue::Number { value } => number_text(*value),
        CellValue::Text { value } => value.clone(),
        CellValue::Bool { value } => (if *value { "TRUE" } else { "FALSE" }).to_owned(),
        CellValue::Error { value } => serde_json::to_value(value).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(),
    }
}

/// Quotes a field that holds the separator, a quote or a line break.
fn quote(field: &str, sep: char) -> String {
    if field.contains([sep, '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

/// The first sheet of `model` as a delimited file (`\n` line ends, the
/// last line ended too), from A1 to the used range's corner.
pub(crate) fn write_delimited(model: &WorkbookModel, sep: char) -> String {
    let Some(sheet) = model.sheets.first() else { return String::new() };
    let Some(used) = sheet.used_range() else { return String::new() };
    let mut out = String::new();
    for r in 0..=used.end.row {
        let mut fields: Vec<String> = (0..=used.end.col)
            .map(|c| sheet.cell(CellRef::new(r, c)).map(|cell| quote(&value_text(&cell.value), sep)).unwrap_or_default())
            .collect();
        // a row's trailing empty fields are dropped, as most writers do
        while fields.len() > 1 && fields.last().is_some_and(String::is_empty) {
            fields.pop();
        }
        out.push_str(&fields.join(&sep.to_string()));
        out.push('\n');
    }
    out
}

/// A legacy `.xls` workbook as `.xlsx` bytes: its sheets' values, dates
/// kept as dates. Formulas, styles and charts are not carried over
/// (office_oxide reads values).
pub(crate) fn xls_to_xlsx(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let doc = office_oxide::xls::XlsDocument::from_reader(std::io::Cursor::new(bytes)).map_err(|e| format!("Couldn't read the .xls file: {e}"))?;
    let mut model = WorkbookModel::default();
    let mut dates: Vec<Vec<CellInput>> = Vec::new();
    for xs in &doc.sheets {
        let mut sheet = Sheet::new(if xs.name.is_empty() { format!("Sheet{}", model.sheets.len() + 1) } else { xs.name.clone() });
        let mut typed = Vec::new();
        for (r, row) in xs.rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                use office_oxide::xls::CellValue as X;
                let at = CellRef::new(r as u32, c as u32);
                let value = match v {
                    X::Empty => continue,
                    X::Number(n) => {
                        // a date shows as an ISO date: typed in, it gets a
                        // date format
                        let shown = xs.display_text(r, c).unwrap_or_default();
                        if is_iso_date(&shown) {
                            typed.push(CellInput { cell: at, input: shown.into_owned() });
                        }
                        CellValue::Number { value: *n }
                    }
                    X::String(s) => CellValue::Text { value: s.clone() },
                    X::Bool(b) => CellValue::Bool { value: *b },
                    X::Error(_) => CellValue::Text { value: v.as_text() },
                };
                sheet.set_cell(at, Cell { value, ..Cell::default() });
            }
        }
        model.sheets.push(sheet);
        dates.push(typed);
    }
    if model.sheets.is_empty() {
        model.sheets.push(Sheet::new("Sheet1"));
    }
    let mut wb = Workbook::from_model(model).map_err(|e| e.to_string())?;
    for (i, typed) in dates.iter().enumerate() {
        if !typed.is_empty() {
            wb.edit_cells(SheetId(i as u32), typed, CalculationOptions::default()).map_err(|e| e.to_string())?;
        }
    }
    wb.save().map_err(|e| e.to_string())
}

/// `2024-01-05` or `2024-01-05T10:30:00` (office_oxide's date text).
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 10 && b[4] == b'-' && b[7] == b'-' && b[..4].iter().chain(&b[5..7]).chain(&b[8..10]).all(u8::is_ascii_digit)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn parses_quotes_separators_and_line_ends() {
        let rows = parse_delimited("a,\"b,c\",\"say \"\"hi\"\"\"\r\n1,\"two\nlines\",\n\"\",x", ',');
        assert_eq!(
            rows,
            vec![
                vec!["a".to_owned(), "b,c".into(), "say \"hi\"".into()],
                vec!["1".into(), "two\nlines".into(), "".into()],
                vec!["".into(), "x".into()],
            ]
        );
        assert_eq!(parse_delimited("a\tb\n", '\t'), vec![vec!["a".to_owned(), "b".into()]]);
        assert_eq!(parse_delimited("\u{feff}x\n", ','), vec![vec!["x".to_owned()]]);
    }

    #[test]
    fn only_plain_numbers_become_numbers() {
        assert_eq!(field_value("42"), CellValue::Number { value: 42.0 });
        assert_eq!(field_value("-1.5"), CellValue::Number { value: -1.5 });
        for text in ["007", "1,234", "2024-01-05", "TRUE", "1.50", "+3", "inf", "NaN", " 4", "1e5"] {
            assert_eq!(field_value(text), CellValue::Text { value: text.to_owned() }, "{text}");
        }
    }

    #[test]
    fn delimited_text_round_trips() {
        let src = "name,qty,note\nwidget,3,\"a, b\"\ngadget,0.25,\"say \"\"hi\"\"\"\n007,,x\n";
        let model = delimited_model(&parse_delimited(src, ','), "s");
        assert_eq!(write_delimited(&model, ','), src);
        let tsv = "a\tb\n1\t2\n";
        assert_eq!(write_delimited(&delimited_model(&parse_delimited(tsv, '\t'), "s"), '\t'), tsv);
    }

    #[test]
    fn numbers_write_their_shortest_form() {
        assert_eq!(number_text(3.0), "3");
        assert_eq!(number_text(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(number_text(1e300).parse::<f64>().unwrap(), 1e300);
        assert!(number_text(1e300).len() < 30);
    }

    /// A legacy `.xls`: a compound file holding a BIFF8 `Workbook` stream
    /// with one sheet of `cells` (numbers, and text through the shared
    /// string table).
    pub(crate) fn xls(sheet: &str, cells: &[(u16, u16, Result<f64, &str>)]) -> Vec<u8> {
        fn rec(out: &mut Vec<u8>, kind: u16, data: &[u8]) {
            out.extend(kind.to_le_bytes());
            out.extend((data.len() as u16).to_le_bytes());
            out.extend(data);
        }
        let bof = |dt: u16| [0x0600u16, dt, 0x0DBB, 0x07CC, 0, 0, 0, 0].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        let strings: Vec<&str> = cells.iter().filter_map(|c| c.2.err()).collect();
        let mut sst = Vec::new();
        sst.extend((strings.len() as u32).to_le_bytes());
        sst.extend((strings.len() as u32).to_le_bytes());
        for s in &strings {
            sst.extend((s.len() as u16).to_le_bytes());
            sst.push(0);
            sst.extend(s.as_bytes());
        }
        let boundsheet_len = 4 + 8 + sheet.len();
        let globals_len = (4 + 16) + (4 + sst.len()) + boundsheet_len + 4;
        let mut w = Vec::new();
        rec(&mut w, 0x0809, &bof(0x0005));
        rec(&mut w, 0x00FC, &sst);
        let mut bs = (globals_len as u32).to_le_bytes().to_vec();
        bs.extend([0, 0, sheet.len() as u8, 0]);
        bs.extend(sheet.as_bytes());
        rec(&mut w, 0x0085, &bs);
        rec(&mut w, 0x000A, &[]);
        assert_eq!(w.len(), globals_len);
        rec(&mut w, 0x0809, &bof(0x0010));
        let mut isst = 0u32;
        for (r, c, v) in cells {
            let mut d = Vec::new();
            d.extend(r.to_le_bytes());
            d.extend(c.to_le_bytes());
            d.extend(0u16.to_le_bytes());
            match v {
                Ok(n) => {
                    d.extend(n.to_le_bytes());
                    rec(&mut w, 0x0203, &d);
                }
                Err(_) => {
                    d.extend(isst.to_le_bytes());
                    isst += 1;
                    rec(&mut w, 0x00FD, &d);
                }
            }
        }
        rec(&mut w, 0x000A, &[]);
        // a stream under 4096 bytes would live in the mini stream
        w.resize(4096, 0);
        // the compound file: its header, then sectors of FAT, directory and the stream (8)
        let mut f = vec![0u8; 512 * (1 + 2 + 8)];
        let put = |f: &mut Vec<u8>, at: usize, b: &[u8]| f[at..at + b.len()].copy_from_slice(b);
        put(&mut f, 0, &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
        put(&mut f, 24, &0x003Eu16.to_le_bytes());
        put(&mut f, 26, &3u16.to_le_bytes());
        put(&mut f, 28, &0xFFFEu16.to_le_bytes());
        put(&mut f, 30, &9u16.to_le_bytes());
        put(&mut f, 32, &6u16.to_le_bytes());
        put(&mut f, 44, &1u32.to_le_bytes()); // FAT sectors
        put(&mut f, 48, &1u32.to_le_bytes()); // first directory sector
        put(&mut f, 56, &4096u32.to_le_bytes());
        put(&mut f, 60, &0xFFFF_FFFEu32.to_le_bytes());
        put(&mut f, 68, &0xFFFF_FFFEu32.to_le_bytes());
        for i in 0..109 {
            put(&mut f, 76 + i * 4, &(if i == 0 { 0u32 } else { 0xFFFF_FFFF }).to_le_bytes());
        }
        let fat = 512;
        for i in 0..128u32 {
            let v = match i {
                0 => 0xFFFF_FFFD,
                1 | 9 => 0xFFFF_FFFE,
                2..=8 => i + 1,
                _ => 0xFFFF_FFFF,
            };
            put(&mut f, fat + i as usize * 4, &v.to_le_bytes());
        }
        let dir = 1024;
        let entry = |f: &mut Vec<u8>, n: usize, name: &str, kind: u8, child: u32, start: u32, size: u32| {
            let at = dir + n * 128;
            let utf16: Vec<u8> = name.encode_utf16().chain([0]).flat_map(|u| u.to_le_bytes()).collect();
            put(f, at, &utf16);
            put(f, at + 64, &(utf16.len() as u16).to_le_bytes());
            f[at + 66] = kind;
            f[at + 67] = 1;
            put(f, at + 68, &0xFFFF_FFFFu32.to_le_bytes());
            put(f, at + 72, &0xFFFF_FFFFu32.to_le_bytes());
            put(f, at + 76, &child.to_le_bytes());
            put(f, at + 116, &start.to_le_bytes());
            put(f, at + 120, &size.to_le_bytes());
        };
        entry(&mut f, 0, "Root Entry", 5, 1, 0xFFFF_FFFE, 0);
        entry(&mut f, 1, "Workbook", 2, 0xFFFF_FFFF, 2, 4096);
        for n in 2..4 {
            let at = dir + n * 128;
            put(&mut f, at + 68, &0xFFFF_FFFFu32.to_le_bytes());
            put(&mut f, at + 72, &0xFFFF_FFFFu32.to_le_bytes());
            put(&mut f, at + 76, &0xFFFF_FFFFu32.to_le_bytes());
        }
        f[1536..].copy_from_slice(&w);
        f
    }

    #[test]
    fn xls_imports_as_xlsx() {
        let bytes = xls("Ledger", &[(0, 0, Err("item")), (0, 1, Err("amount")), (1, 0, Err("rent")), (1, 1, Ok(1250.5))]);
        let xlsx = xls_to_xlsx(&bytes).unwrap();
        let wb = Workbook::open(&xlsx).unwrap();
        assert_eq!(wb.model().sheets[0].name, "Ledger");
        let at = |r, c| wb.model().sheets[0].cell(CellRef::new(r, c)).map(|c| c.value.clone());
        assert_eq!(at(0, 1), Some(CellValue::Text { value: "amount".into() }));
        assert_eq!(at(1, 1), Some(CellValue::Number { value: 1250.5 }));
        assert!(xls_to_xlsx(b"not a workbook").is_err());
    }

    #[test]
    fn iso_dates() {
        assert!(is_iso_date("2024-01-05"));
        assert!(is_iso_date("2024-01-05T10:00:00"));
        assert!(!is_iso_date("45000"));
        assert!(!is_iso_date("2024/01/05"));
    }
}
