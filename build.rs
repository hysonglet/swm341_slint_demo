use std::path::{Path, PathBuf};

fn main() {
    // Compile every `.slint` file into Rust. `EmbedForSoftwareRenderer` keeps
    // images/fonts in the binary in the raw formats the software renderer wants
    // (no decoding/parsing at runtime).
    slint_build::compile_with_config(
        "ui/main.slint",
        slint_build::CompilerConfiguration::new()
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer),
    )
    .unwrap();

    // `cortex-m-rt` picks up `memory.x` from the linker search path. Keep it
    // identical to the layout the HAL ships (they would otherwise race for the
    // same `-L` entry).
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    // The gallery: every picture is turned into raw RGB565 and linked into the
    // `.sfc_xip` section, i.e. into the SPI NOR window, so the pictures are part
    // of this ELF without costing a byte of the 512K internal flash.
    build_gallery(&out);

    println!("cargo:rerun-if-changed=ui/main.slint");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=assets");
    println!("cargo:rerun-if-changed=build.rs");
}

/// Produce `$OUT_DIR/gallery.bin` and the Rust module that links it in.
fn build_gallery(out: &Path) {
    let bin = out.join("gallery.bin");
    let size = std::env::var("GALLERY_SIZE").unwrap_or_else(|_| "480x320".to_string());
    let synthetic =
        std::env::var("GALLERY_SYNTHETIC").unwrap_or_else(|_| "3".to_string());
    let raster = rasterize_svgs(Path::new("assets"));

    let mut command = std::process::Command::new("python3");
    command.args([
        "tools/make_gallery.py",
        "--assets",
        "assets",
        "--assets",
        raster.to_str().unwrap(),
        "--out",
        bin.to_str().unwrap(),
        "--size",
        &size,
        "--synthetic",
        &synthetic,
    ]);
    let status = command
        .status()
        .expect("python3 is needed to build the gallery (see tools/make_gallery.py)");
    assert!(status.success(), "tools/make_gallery.py failed");
    println!("cargo:rerun-if-changed=tools/make_gallery.py");

    let len = std::fs::metadata(&bin).unwrap().len();
    // `include_bytes!` resolves relative to the including file, so hand over the
    // absolute path; the section attribute puts the bytes in the SFC window.
    // `#[repr(align(4))]` is not allowed on a `static`, hence the wrapper: the
    // AHB window only serves word accesses, so the image must start on one.
    let module = format!(
        "/// Gallery image: {len} bytes, linked into the SFC window.\n\
         #[repr(align(4))]\n\
         pub struct Gallery(pub [u8; {len}]);\n\
         #[link_section = \".sfc_xip.gallery\"]\n\
         pub static GALLERY: Gallery = Gallery(*include_bytes!(\"{}\"));\n",
        bin.display()
    );
    std::fs::write(out.join("gallery.rs"), module).unwrap();
    println!("cargo:rustc-env=GALLERY_LEN={}", len);
}

/// Render every `*.svg` in `assets/` to a PNG the packer can read.
///
/// Vector artwork is rasterised at 2x and downscaled to the target size by the
/// packer, which keeps the edges clean. Returns the directory holding the PNGs.
fn rasterize_svgs(assets: &Path) -> PathBuf {
    let out = assets.join("build").join("raster");
    std::fs::create_dir_all(&out).unwrap();

    let entries = match std::fs::read_dir(assets) {
        Ok(entries) => entries,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("svg") {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());

        let data = std::fs::read(&path).unwrap();
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default())
            .unwrap_or_else(|e| panic!("{}: {}", path.display(), e));

        // 2x supersampling; the packer scales it down to the gallery size.
        let scale = 2.0f32;
        let size = tree.size();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(
            (size.width() * scale) as u32,
            (size.height() * scale) as u32,
        )
        .expect("svg too large");
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );

        let name = path.file_stem().unwrap().to_str().unwrap();
        std::fs::write(out.join(format!("{name}.png")), pixmap.encode_png().unwrap())
            .unwrap();
    }
    out
}
