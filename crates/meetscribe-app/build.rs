// exe 的圖示（檔案總管、工作列、捷徑）：工具/make_icons.py 產的 packaging/icon.ico。
// 2026-10-31 使用者：「為什麼電腦程式沒有圖示 幫我重新設計圖示電腦與手機要一致」
fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/icon.ico");
        res.set("FileDescription", "語音轉錄");
        res.set("ProductName", "語音轉錄");
        if let Err(e) = res.compile() {
            // 沒有 rc.exe（Windows SDK）時照樣建得出來，只是沒有圖示
            println!("cargo:warning=沒嵌入圖示：{e}");
        }
    }
}
