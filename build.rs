//! Embeds the icon and version information in Windows executables.

fn main() {
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=packaging/windows/fastsapp.ico");
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("packaging/windows/fastsapp.ico")
            .set("ProductName", "FastsApp")
            .set("FileDescription", "FastsApp");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=Windows resources not embedded: {error}");
        }
    }
}
