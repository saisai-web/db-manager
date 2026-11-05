//! 起動オプション。`psql -c` のように、起動と同時に接続を開いて SQL を実行できる。
//!
//! ```text
//! sqltool --import-connections FILE   接続設定(エクスポートした JSON)を取り込む
//! sqltool --open NAME                 起動時に、その名前の接続を開く
//! sqltool --run "SQL"                 接続を開いた後、SQL を全文実行する(--open と併用)
//! sqltool --explain "SQL"             接続を開いた後、その SQL の実行計画(実測つき)を表示する(--open と併用)
//! sqltool --table スキーマ.表名        接続を開いた後、そのテーブルを開く(--open と併用)
//! sqltool --er スキーマ名              接続を開いた後、そのスキーマの ER 図を表示する(--open と併用)
//! sqltool --design FILE               起動時に、ER 設計でそのファイル(.erd)を開く
//! ```

use std::path::PathBuf;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Launch {
    pub import: Option<PathBuf>,
    pub open: Option<String>,
    pub run: Option<String>,
    pub table: Option<String>,
    /// 接続を開いた後、この SQL の実行計画(実測つき)を表示する
    pub explain: Option<String>,
    /// 接続を開いた後、このスキーマの ER 図を表示する
    pub er: Option<String>,
    /// 起動時に ER 設計で開くファイル
    pub design: Option<PathBuf>,
}

impl Launch {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut out = Self::default();
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            let mut value =
                |name: &str| it.next().ok_or_else(|| format!("{name} には値が必要です"));
            match a.as_str() {
                "--import-connections" => {
                    out.import = Some(PathBuf::from(value("--import-connections")?))
                }
                "--open" => out.open = Some(value("--open")?),
                "--run" => out.run = Some(value("--run")?),
                "--table" => out.table = Some(value("--table")?),
                "--explain" => out.explain = Some(value("--explain")?),
                "--er" => out.er = Some(value("--er")?),
                "--design" => out.design = Some(PathBuf::from(value("--design")?)),
                other => return Err(format!("不明なオプション: {other}")),
            }
        }
        let needs_open =
            out.run.is_some() || out.table.is_some() || out.explain.is_some() || out.er.is_some();
        if needs_open && out.open.is_none() {
            return Err(
                "--run、--table、--explain、--er は --open と一緒に指定してください".into(),
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Launch, String> {
        Launch::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_arguments_means_a_plain_start() {
        assert_eq!(parse(&[]).unwrap(), Launch::default());
    }

    #[test]
    fn options_are_parsed() {
        let l = parse(&[
            "--import-connections",
            "a.json",
            "--open",
            "dev",
            "--run",
            "select 1",
        ])
        .unwrap();
        assert_eq!(l.import, Some(PathBuf::from("a.json")));
        assert_eq!(l.open.as_deref(), Some("dev"));
        assert_eq!(l.run.as_deref(), Some("select 1"));
    }

    #[test]
    fn the_designer_needs_no_connection() {
        let l = parse(&["--design", "shop.erd"]).unwrap();
        assert_eq!(l.design, Some(PathBuf::from("shop.erd")));
        assert!(l.open.is_none());
        assert!(parse(&["--design"]).is_err());
    }

    #[test]
    fn mistakes_are_reported() {
        assert!(parse(&["--open"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert!(
            parse(&["--table", "public.t"])
                .unwrap_err()
                .contains("--open")
        );
        assert!(parse(&["--er", "public"]).unwrap_err().contains("--open"));
        assert!(
            parse(&["--run", "select 1"])
                .unwrap_err()
                .contains("--open")
        );
    }
}
