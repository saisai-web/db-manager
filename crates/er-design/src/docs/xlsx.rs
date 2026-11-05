//! 設計書の Excel(.xlsx)。表紙・テーブル一覧・リレーション一覧と、表ごとのシートを作る。
//! 一覧の表名から各シートへ、各シートから一覧へ、ハイパーリンクで行き来できる。

use rust_xlsxwriter::{
    Color, Format, FormatAlign, FormatBorder, Url, Workbook, Worksheet, XlsxError,
};

use super::rows::{column_rows, constraint_rows, relation_rows};
use super::{DocOptions, doc_title};
use crate::model::{Design, Table};

const NAVY: u32 = 0x1E3A5F;
const COVER: &str = "表紙";
const LIST: &str = "テーブル一覧";
const RELATIONS: &str = "リレーション一覧";

struct Styles {
    title: Format,
    head: Format,
    cell: Format,
    center: Format,
    label: Format,
    link: Format,
    section: Format,
}

impl Styles {
    fn new() -> Self {
        let border = |f: Format| {
            f.set_border(FormatBorder::Thin)
                .set_border_color(Color::RGB(0xB8C2D0))
        };
        let cell = border(Format::new())
            .set_text_wrap()
            .set_align(FormatAlign::Top);
        Self {
            title: Format::new()
                .set_bold()
                .set_font_size(18)
                .set_font_color(Color::RGB(NAVY)),
            head: border(Format::new())
                .set_bold()
                .set_font_color(Color::White)
                .set_background_color(Color::RGB(NAVY))
                .set_align(FormatAlign::Center)
                .set_align(FormatAlign::VerticalCenter)
                .set_text_wrap(),
            center: cell.clone().set_align(FormatAlign::Center),
            label: border(Format::new())
                .set_bold()
                .set_background_color(Color::RGB(0xE8EEF6))
                .set_align(FormatAlign::Top),
            link: cell
                .clone()
                .set_font_color(Color::RGB(0x2563EB))
                .set_underline(rust_xlsxwriter::FormatUnderline::Single),
            section: Format::new()
                .set_bold()
                .set_font_size(12)
                .set_font_color(Color::RGB(NAVY)),
            cell,
        }
    }
}

/// Excel のシート名にする。使えない文字は `_`、31 文字まで、重複は連番で避ける(大文字小文字は区別されない)。
pub fn sheet_names(d: &Design) -> Vec<String> {
    let mut used: Vec<String> = [COVER, LIST, RELATIONS]
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    d.tables
        .iter()
        .map(|t| {
            let base: String = t
                .name
                .chars()
                .map(|c| if "[]:*?/\\".contains(c) { '_' } else { c })
                .collect::<String>()
                .trim_matches('\'')
                .chars()
                .take(31)
                .collect();
            let base = if base.is_empty() {
                "_".to_string()
            } else {
                base
            };
            let mut name = base.clone();
            let mut n = 2;
            while used.contains(&name.to_lowercase()) {
                let suffix = format!("_{n}");
                name = format!(
                    "{}{suffix}",
                    base.chars().take(31 - suffix.len()).collect::<String>()
                );
                n += 1;
            }
            used.push(name.to_lowercase());
            name
        })
        .collect()
}

fn link_to(sheet: &str, text: &str) -> Url {
    Url::new(format!("internal:'{}'!A1", sheet.replace('\'', "''"))).set_text(text)
}

fn header(
    ws: &mut Worksheet,
    row: u32,
    first_col: u16,
    names: &[&str],
    st: &Styles,
) -> Result<(), XlsxError> {
    for (i, n) in names.iter().enumerate() {
        ws.write_string_with_format(row, first_col + i as u16, *n, &st.head)?;
    }
    Ok(())
}

fn widths(ws: &mut Worksheet, widths: &[f64]) -> Result<(), XlsxError> {
    for (i, w) in widths.iter().enumerate() {
        ws.set_column_width(i as u16, *w)?;
    }
    Ok(())
}

fn print_setup(ws: &mut Worksheet) {
    ws.set_landscape()
        .set_paper_size(9)
        .set_print_fit_to_pages(1, 0);
}

fn cover(ws: &mut Worksheet, d: &Design, opts: &DocOptions, st: &Styles) -> Result<(), XlsxError> {
    ws.set_name(COVER)?;
    widths(ws, &[16.0, 70.0])?;
    ws.write_string_with_format(0, 0, doc_title(d), &st.title)?;
    ws.set_row_height(0, 30)?;
    let relations = d.tables.iter().map(|t| t.foreign_keys.len()).sum::<usize>();
    let rows = [
        ("バージョン", d.version.clone()),
        ("作成者", d.author.clone()),
        ("作成日", opts.date.clone()),
        ("表の数", d.tables.len().to_string()),
        ("外部キーの数", relations.to_string()),
        ("概要", d.description.clone()),
    ];
    let mut r = 2;
    for (label, value) in rows.iter().filter(|(_, v)| !v.is_empty()) {
        ws.write_string_with_format(r, 0, *label, &st.label)?;
        ws.write_string_with_format(r, 1, value, &st.cell)?;
        r += 1;
    }
    r += 1;
    ws.write_string_with_format(r, 0, "シート", &st.section)?;
    for (i, name) in [LIST, RELATIONS].iter().enumerate() {
        ws.write_url_with_format(r + 1 + i as u32, 0, link_to(name, name), &st.link)?;
    }
    print_setup(ws);
    Ok(())
}

fn table_list(
    ws: &mut Worksheet,
    d: &Design,
    sheets: &[String],
    st: &Styles,
) -> Result<(), XlsxError> {
    ws.set_name(LIST)?;
    widths(ws, &[6.0, 28.0, 24.0, 44.0, 8.0, 24.0])?;
    header(
        ws,
        0,
        0,
        &["No", "物理名", "論理名", "概要", "列数", "主キー"],
        st,
    )?;
    for (i, t) in d.tables.iter().enumerate() {
        let r = i as u32 + 1;
        ws.write_number_with_format(r, 0, (i + 1) as f64, &st.center)?;
        ws.write_url_with_format(r, 1, link_to(&sheets[i], &t.name), &st.link)?;
        ws.write_string_with_format(r, 2, &t.logical, &st.cell)?;
        ws.write_string_with_format(r, 3, &t.comment, &st.cell)?;
        ws.write_number_with_format(r, 4, t.columns.len() as f64, &st.center)?;
        ws.write_string_with_format(r, 5, t.primary_key().join(", "), &st.cell)?;
    }
    ws.set_freeze_panes(1, 0)?;
    print_setup(ws);
    Ok(())
}

fn relations(ws: &mut Worksheet, d: &Design, st: &Styles) -> Result<(), XlsxError> {
    ws.set_name(RELATIONS)?;
    widths(ws, &[22.0, 22.0, 28.0, 40.0, 16.0, 16.0, 16.0])?;
    header(
        ws,
        0,
        0,
        &[
            "親の表",
            "子の表",
            "外部キー名",
            "列",
            "多重度(親 : 子)",
            "削除時",
            "更新時",
        ],
        st,
    )?;
    for (i, r) in relation_rows(d).iter().enumerate() {
        let row = i as u32 + 1;
        for (c, v) in [&r.parent, &r.child, &r.name, &r.columns]
            .iter()
            .enumerate()
        {
            ws.write_string_with_format(row, c as u16, *v, &st.cell)?;
        }
        ws.write_string_with_format(row, 4, &r.cardinality, &st.center)?;
        ws.write_string_with_format(row, 5, r.on_delete, &st.cell)?;
        ws.write_string_with_format(row, 6, r.on_update, &st.cell)?;
    }
    ws.set_freeze_panes(1, 0)?;
    print_setup(ws);
    Ok(())
}

fn mark(on: bool) -> &'static str {
    if on { "○" } else { "" }
}

fn table_sheet(
    ws: &mut Worksheet,
    d: &Design,
    t: &Table,
    name: &str,
    st: &Styles,
) -> Result<(), XlsxError> {
    ws.set_name(name)?;
    widths(
        ws,
        &[6.0, 22.0, 26.0, 18.0, 10.0, 6.0, 24.0, 6.0, 6.0, 16.0, 36.0],
    )?;
    ws.write_url_with_format(
        0,
        0,
        link_to(LIST, "← テーブル一覧"),
        &Format::new()
            .set_font_color(Color::RGB(0x2563EB))
            .set_underline(rust_xlsxwriter::FormatUnderline::Single),
    )?;
    for (i, (label, value)) in [
        ("物理名", &t.name),
        ("論理名", &t.logical),
        ("概要", &t.comment),
    ]
    .iter()
    .enumerate()
    {
        let r = 2 + i as u32;
        ws.merge_range(r, 0, r, 1, label, &st.label)?;
        ws.merge_range(r, 2, r, 10, value, &st.cell)?;
    }
    ws.set_row_height(4, 36)?;
    ws.write_string_with_format(6, 0, "列定義", &st.section)?;
    header(
        ws,
        7,
        0,
        &[
            "No",
            "論理名",
            "物理名",
            "型",
            "NOT NULL",
            "PK",
            "FK(参照先)",
            "UQ",
            "連番",
            "既定値",
            "備考",
        ],
        st,
    )?;
    let mut r = 8;
    for c in column_rows(d, t) {
        ws.write_number_with_format(r, 0, c.no as f64, &st.center)?;
        let cells = [&c.logical, &c.name, &c.ty];
        for (i, v) in cells.iter().enumerate() {
            ws.write_string_with_format(r, 1 + i as u16, *v, &st.cell)?;
        }
        ws.write_string_with_format(r, 4, mark(c.not_null), &st.center)?;
        ws.write_string_with_format(r, 5, mark(c.primary), &st.center)?;
        ws.write_string_with_format(r, 6, &c.foreign, &st.cell)?;
        ws.write_string_with_format(r, 7, mark(c.unique), &st.center)?;
        ws.write_string_with_format(r, 8, mark(c.auto), &st.center)?;
        ws.write_string_with_format(r, 9, &c.default, &st.cell)?;
        ws.write_string_with_format(r, 10, &c.comment, &st.cell)?;
        r += 1;
    }
    let cons = constraint_rows(d, t);
    if !cons.is_empty() {
        r += 1;
        ws.write_string_with_format(r, 0, "制約・索引", &st.section)?;
        r += 1;
        header(ws, r, 1, &["種類", "名前"], st)?;
        ws.merge_range(r, 3, r, 5, "列", &st.head)?;
        ws.write_string_with_format(r, 6, "参照先", &st.head)?;
        ws.merge_range(r, 7, r, 8, "削除時", &st.head)?;
        ws.write_string_with_format(r, 9, "更新時", &st.head)?;
        for c in cons {
            r += 1;
            ws.write_string_with_format(r, 1, c.kind, &st.cell)?;
            ws.write_string_with_format(r, 2, &c.name, &st.cell)?;
            ws.merge_range(r, 3, r, 5, &c.columns, &st.cell)?;
            ws.write_string_with_format(r, 6, &c.target, &st.cell)?;
            ws.merge_range(r, 7, r, 8, c.on_delete, &st.cell)?;
            ws.write_string_with_format(r, 9, c.on_update, &st.cell)?;
        }
    }
    ws.set_freeze_panes(8, 0)?;
    print_setup(ws);
    Ok(())
}

/// .xlsx ファイルの中身。
pub fn xlsx(d: &Design, opts: &DocOptions) -> Result<Vec<u8>, String> {
    build(d, opts).map_err(|e| e.to_string())
}

fn build(d: &Design, opts: &DocOptions) -> Result<Vec<u8>, XlsxError> {
    let st = Styles::new();
    let sheets = sheet_names(d);
    let mut book = Workbook::new();
    cover(book.add_worksheet(), d, opts, &st)?;
    table_list(book.add_worksheet(), d, &sheets, &st)?;
    relations(book.add_worksheet(), d, &st)?;
    for (t, name) in d.tables.iter().zip(&sheets) {
        table_sheet(book.add_worksheet(), d, t, name, &st)?;
    }
    book.save_to_buffer()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse;

    #[test]
    fn sheet_names_are_valid_unique_and_short() {
        let long = "x".repeat(40);
        let d = parse(&format!(
            "a/b\n  i int\n{long}\n  i int\nA_B\n  i int\n表紙\n  i int\n"
        ))
        .unwrap();
        // 表の名前に '/' は書けるが、シート名には使えない
        let names = sheet_names(&d);
        assert_eq!(names[0], "a_b");
        assert_eq!(names[1].chars().count(), 31);
        assert_eq!(names[2], "A_B_2", "case-insensitive duplicates: {names:?}");
        assert_eq!(names[3], "表紙_2", "must not collide with the fixed sheets");
        assert!(
            names
                .iter()
                .all(|n| !n.contains(['[', ']', ':', '*', '?', '/', '\\']))
        );
    }

    #[test]
    fn the_workbook_is_a_zip_with_one_sheet_per_table_plus_three() {
        let d =
            parse("a \"顧客\"\n  id int pk\nb\n  id int pk\n  a_id int -> a.id\n  index (a_id)\n")
                .unwrap();
        let bytes = xlsx(
            &d,
            &DocOptions {
                date: "2026-11-04".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(&bytes[..2], b"PK");
        let text = String::from_utf8_lossy(&bytes);
        for sheet in 1..=5 {
            assert!(
                text.contains(&format!("xl/worksheets/sheet{sheet}.xml")),
                "sheet{sheet}"
            );
        }
        assert!(!text.contains("xl/worksheets/sheet6.xml"));
    }

    #[test]
    fn an_empty_design_and_odd_names_still_produce_a_workbook() {
        assert!(xlsx(&Design::default(), &DocOptions::default()).is_ok());
        // テキスト記法では書けない名前も、モデルとしては作れる
        let mut d = parse("t\n  i int\n").unwrap();
        d.tables[0].name = "it's".into();
        assert_eq!(sheet_names(&d), ["it's"]);
        assert!(xlsx(&d, &DocOptions::default()).is_ok());
    }
}
