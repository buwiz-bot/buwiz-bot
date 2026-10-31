//! A workbook shaped like the showcase one ("Q3 Sales Dashboard"), built
//! in code for the tests: a title merged over A1:E1, revenue by region in
//! C4:C8 and its bold currency total in C9 (`$1,504,750.00`, too wide for a
//! default column: it shows `#####`), a link in A11 to `Notes!B3`, and a
//! chart framed over H2:O17.

const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

const CHART: &str = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>Revenue by region</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title><c:plotArea><c:barChart><c:barDir val="col"/><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:v>Revenue</c:v></c:tx><c:cat><c:strRef><c:f>Dashboard!$A$4:$A$8</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Dashboard!$C$4:$C$8</c:f></c:numRef></c:val></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"#;

/// The regions and their revenue (their sum is the total's cached value).
const REGIONS: [(&str, u32); 5] = [("North", 412_300), ("South", 398_150), ("East", 365_400), ("West", 228_900), ("Online", 100_000)];

pub(crate) fn showcase() -> Vec<u8> {
    let mut rows = String::from(r#"<row r="1"><c r="A1" t="inlineStr"><is><t>Q3 Sales Dashboard</t></is></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>Region</t></is></c><c r="C3" t="inlineStr"><is><t>Revenue</t></is></c></row>"#);
    for (i, (name, v)) in REGIONS.iter().enumerate() {
        let r = i + 4;
        rows.push_str(&format!(r#"<row r="{r}"><c r="A{r}" t="inlineStr"><is><t>{name}</t></is></c><c r="C{r}" s="1"><v>{v}</v></c></row>"#));
    }
    let total: u32 = REGIONS.iter().map(|r| r.1).sum();
    rows.push_str(&format!(r#"<row r="9"><c r="A9" t="inlineStr"><is><t>Total</t></is></c><c r="C9" s="2"><f>SUM(C4:C8)</f><v>{total}</v></c></row>"#));
    rows.push_str(r#"<row r="11"><c r="A11" t="inlineStr"><is><t>Details</t></is></c></row>"#);
    let sheet = format!(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="{REL}"><sheetData>{rows}</sheetData><mergeCells count="1"><mergeCell ref="A1:E1"/></mergeCells><hyperlinks><hyperlink ref="A11" location="Notes!B3" display="Details" tooltip="Open the notes"/></hyperlinks><drawing r:id="d1"/></worksheet>"#
    );
    let notes = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="3"><c r="B3" t="inlineStr"><is><t>Notes</t></is></c></row></sheetData></worksheet>"#.to_owned();
    let styles = r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="&quot;$&quot;#,##0.00"/></numFmts><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf/></cellStyleXfs><cellXfs count="3"><xf numFmtId="0" fontId="0"/><xf numFmtId="164" fontId="0" applyNumberFormat="1"/><xf numFmtId="164" fontId="1" applyNumberFormat="1" applyFont="1"/></cellXfs></styleSheet>"#.to_owned();
    // the frame: from H2 (col 7, row 1) to the top left of P18
    let drawing = r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><xdr:twoCellAnchor><xdr:from><xdr:col>7</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>15</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>17</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Revenue by region"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart r:id="c1"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#.to_owned();
    let rels = |items: &[(&str, &str, &str)]| {
        let mut x = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
        for (id, kind, target) in items {
            x.push_str(&format!(r#"<Relationship Id="{id}" Type="{REL}/{kind}" Target="{target}"/>"#));
        }
        x + "</Relationships>"
    };
    let parts = vec![
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/drawings/drawing1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/><Override PartName="/xl/charts/chart1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/></Types>"#.to_owned(),
        ),
        ("_rels/.rels", rels(&[("rId1", "officeDocument", "xl/workbook.xml")])),
        (
            "xl/workbook.xml",
            format!(r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="{REL}"><sheets><sheet name="Dashboard" sheetId="1" r:id="r1"/><sheet name="Notes" sheetId="2" r:id="r2"/></sheets></workbook>"#),
        ),
        ("xl/_rels/workbook.xml.rels", rels(&[("r1", "worksheet", "worksheets/sheet1.xml"), ("r2", "worksheet", "worksheets/sheet2.xml"), ("r3", "styles", "styles.xml")])),
        ("xl/styles.xml", styles),
        ("xl/worksheets/sheet1.xml", sheet),
        ("xl/worksheets/sheet2.xml", notes),
        ("xl/worksheets/_rels/sheet1.xml.rels", rels(&[("d1", "drawing", "../drawings/drawing1.xml")])),
        ("xl/drawings/drawing1.xml", drawing),
        ("xl/drawings/_rels/drawing1.xml.rels", rels(&[("c1", "chart", "../charts/chart1.xml")])),
        ("xl/charts/chart1.xml", CHART.to_owned()),
    ];
    let parts: Vec<(String, Vec<u8>)> = parts.into_iter().map(|(p, x)| (p.to_owned(), x.into_bytes())).collect();
    ooxml_opc::rezip_parts(&parts).unwrap()
}
