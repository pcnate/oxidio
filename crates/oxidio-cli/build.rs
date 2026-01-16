fn main() {
    // Check TARGET (what we're compiling for), not HOST (where we're compiling)
    let target = std::env::var( "CARGO_CFG_TARGET_OS" ).unwrap_or_default();
    if target == "windows" {
        let mut res = winres::WindowsResource::new();

        // For cross-compilation from Linux, use the mingw windres
        if std::env::var( "CARGO_CFG_TARGET_ARCH" ).unwrap_or_default() == "x86_64" {
            res.set_windres_path( "x86_64-w64-mingw32-windres" );
        }

        res.set( "ProductName", "Oxidio" );
        res.set( "FileDescription", "Oxidio Music Player" );
        res.set( "OriginalFilename", "oxidio.exe" );
        res.compile().expect( "Failed to compile Windows resources" );
    }
}
