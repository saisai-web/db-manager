//! Windows の実行ファイルに、アイコンとバージョン情報を埋め込む(Mac では何もしない)。

fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "SQL Tool");
        res.set("FileDescription", "SQL Tool");
        res.compile().expect("Windows のリソースを埋め込めません");
    }
}
