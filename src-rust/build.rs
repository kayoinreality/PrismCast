fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        if std::path::Path::new("build/icon.ico").exists() {
            res.set_icon("build/icon.ico");
        }
        res.set("ProductName", "PrismCast Studio");
        res.set(
            "FileDescription",
            "PrismCast Studio — Estúdio de Transmissão 100% Nativo em Rust",
        );
        res.set("LegalCopyright", "MIT License — PrismCast Contributors");
        let _ = res.compile();
    }
}
