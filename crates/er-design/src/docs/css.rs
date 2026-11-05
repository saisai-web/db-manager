//! 設計書(HTML)のスタイル。1 ファイルで完結させるため、HTML に埋め込む。印刷にも向く。

pub const CSS: &str = r#"
:root{--fg:#0f172a;--muted:#64748b;--line:#d5dce6;--head:#1e3a5f;--soft:#f1f5f9;--accent:#2563eb}
*{box-sizing:border-box}
body{margin:0;color:var(--fg);background:#fff;font:14px/1.7 -apple-system,'Hiragino Sans','Yu Gothic UI','Noto Sans JP',Meiryo,sans-serif}
.page{max-width:1180px;margin:0 auto;padding:32px 28px 80px}
header.cover{border-bottom:3px solid var(--head);padding-bottom:16px;margin-bottom:24px}
header.cover h1{margin:0 0 6px;font-size:28px;color:var(--head)}
.meta{display:flex;flex-wrap:wrap;gap:6px 24px;color:var(--muted);font-size:13px}
.meta b{color:var(--fg);font-weight:600}
h2{font-size:20px;margin:40px 0 12px;padding-left:10px;border-left:5px solid var(--accent)}
h3{font-size:17px;margin:0 0 4px}
h3 small{font-weight:400;color:var(--muted);margin-left:8px}
nav.toc ol{columns:3;column-gap:28px;margin:0;padding-left:20px}
nav.toc li{break-inside:avoid}
a{color:var(--accent);text-decoration:none}a:hover{text-decoration:underline}
table{border-collapse:collapse;width:100%;margin:8px 0 14px;font-size:13px}
th,td{border:1px solid var(--line);padding:4px 8px;vertical-align:top;text-align:left}
th{background:var(--head);color:#fff;font-weight:600;white-space:nowrap}
tbody tr:nth-child(even) td{background:#f8fafc}
td.c{text-align:center;white-space:nowrap}
td.mono,.mono{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:12.5px}
td.no{text-align:right;color:var(--muted);width:2.5em}
.desc{white-space:pre-wrap}
section.table{padding:14px 0 6px;border-top:1px solid var(--line)}
.refs{color:var(--muted);font-size:12.5px;margin:2px 0 6px}
.diagram{overflow:auto;border:1px solid var(--line);border-radius:8px;background:#fff;padding:6px}
.diagram svg{display:block;max-width:none}
h4{margin:10px 0 0;font-size:13px;color:var(--muted)}
pre.sql{background:#0f172a;color:#e2e8f0;padding:14px 16px;border-radius:8px;overflow:auto;font-size:12.5px;line-height:1.55}
.tag{display:inline-block;padding:0 6px;border-radius:4px;background:var(--soft);color:var(--muted);font-size:12px}
footer{margin-top:48px;color:var(--muted);font-size:12px;border-top:1px solid var(--line);padding-top:10px}
@media print{
  .page{max-width:none;padding:0}
  section.table{break-inside:avoid}
  .diagram{overflow:visible}
  pre.sql{white-space:pre-wrap;background:#fff;color:#000;border:1px solid var(--line)}
  th{background:#e5e7eb!important;color:#000!important;-webkit-print-color-adjust:exact}
  a{color:inherit}
}
"#;
