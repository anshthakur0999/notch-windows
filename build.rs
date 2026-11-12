fn main() {
    // Embed the icon and the application manifest (Per-Monitor-V2 DPI, Win10/11 compat).
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        // Icon ids: 1 = app icon (Explorer, Start menu; must stay the lowest id), 2 / 3 = tray
        // icon for a dark / light taskbar (picked at run time in shell.rs).
        res.set_icon("res/icon.ico");
        res.set_icon_with_id("res/tray-dark.ico", "2");
        res.set_icon_with_id("res/tray-light.ico", "3");
        res.set_manifest_file("res/app.manifest");
        res.set("ProductName", "Notch");
        res.set("FileDescription", "Notch - floating pill with notepad and clipboard history");
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed resources: {e}");
        }
    }
    println!("cargo:rerun-if-changed=res/app.manifest");
    println!("cargo:rerun-if-changed=res/icon.ico");
    println!("cargo:rerun-if-changed=res/tray-dark.ico");
    println!("cargo:rerun-if-changed=res/tray-light.ico");
    println!("cargo:rerun-if-changed=build.rs");
}
