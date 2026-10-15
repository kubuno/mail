//! Render every Kubuno Mail screen to PNG files, exactly as the device shells show them (same layout + raster code),
//! for a quick look without an emulator or a phone.
//!
//! `cargo run -p kubuno-mail-mobile-common --example render_views -- <font.ttf> <out-dir> [width height]`

use std::path::PathBuf;

use kubuno_mobile_render::{render_view, FontHandle};
use kubuno_mail_mobile_common::{setup, VIEWS};

fn main() {
    let mut args = std::env::args().skip(1);
    let font_path = args.next().expect("usage: render_views <font.ttf> <out-dir> [width height]");
    let out = PathBuf::from(args.next().expect("usage: render_views <font.ttf> <out-dir> [width height]"));
    let width: u32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(1080);
    let height: u32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(2340);
    let mut font = FontHandle::from_bytes(std::fs::read(&font_path).expect("read font"));
    // Load a bold sibling for real-bold preview (e.g. arial.ttf -> arialbd.ttf), else bold is faked.
    let reg = PathBuf::from(&font_path);
    let stem = reg.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    for name in [format!("{stem}bd.ttf"), format!("{stem}-Bold.ttf"), format!("{stem} Bold.ttf")] {
        let cand = reg.with_file_name(name);
        if let Ok(b) = std::fs::read(&cand) {
            println!("bold: {}", cand.display());
            font = font.with_bold(b);
            break;
        }
    }
    std::fs::create_dir_all(&out).expect("create out dir");

    // Register what the screens need, as the shell does before the first frame.
    setup();
    for (i, src) in VIEWS.iter().enumerate() {
        let t = std::time::Instant::now();
        let pm = render_view(src, width, height, Some(&font), None).expect("render");
        let path = out.join(format!("mail-{i:02}.png"));
        pm.save_png(&path).expect("save png");
        println!("mail-{i:02}: {:?}", t.elapsed());
    }
}
