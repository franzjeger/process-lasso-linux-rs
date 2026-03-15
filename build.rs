fn main() {
    let icon_path = "assets/icon.png";
    println!("cargo:rerun-if-changed={icon_path}");

    let file = std::fs::File::open(icon_path).expect("assets/icon.png not found — run rsvg-convert first");
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().expect("failed to decode assets/icon.png");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("failed to read PNG frame");

    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 0xff])
            .collect(),
        other => panic!("unsupported PNG color type: {other:?}"),
    };

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = std::path::Path::new(&out_dir).join("icon_rgba.bin");
    std::fs::write(&out_path, &rgba).expect("failed to write icon_rgba.bin");

    // Emit width/height as env vars for main.rs
    println!("cargo:rustc-env=ICON_W={}", info.width);
    println!("cargo:rustc-env=ICON_H={}", info.height);
}
