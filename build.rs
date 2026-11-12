fn main() {
    // Embed the icon and the application manifest (Per-Monitor-V2 DPI, Win10/11 compat).
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("res/icon.ico");
        res.set_manifest_file("res/app.manifest");
        res.set("ProductName", "Notch");
        res.set("FileDescription", "Notch - floating pill with notepad and clipboard history");
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed resources: {e}");
        }
    }
    println!("cargo:rerun-if-changed=res/app.manifest");
    println!("cargo:rerun-if-changed=res/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
}
