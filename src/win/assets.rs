//! Portable shortcut images imported beside the executable.
use std::io;
use std::path::{Path, PathBuf};
pub fn root() -> PathBuf {
    super::registry::executable()
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf()
}
pub fn writable_root() -> PathBuf {
    let beside_exe = root();
    if std::fs::create_dir_all(beside_exe.join("icons")).is_ok() {
        beside_exe
    } else {
        crate::config::paths::data_dir()
    }
}
pub fn resolve(path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        let beside_exe = root().join(p);
        if beside_exe.exists() {
            beside_exe
        } else {
            crate::config::paths::data_dir().join(p)
        }
    }
}
pub fn decode(path: &Path) -> io::Result<image::RgbaImage> {
    let bytes = std::fs::read(path)?;
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    {
        let mut options = resvg::usvg::Options {
            resources_dir: path.parent().map(Path::to_path_buf),
            ..Default::default()
        };
        options.fontdb_mut().load_system_fonts();
        let tree = resvg::usvg::Tree::from_data(&bytes, &options).map_err(io::Error::other)?;
        let size = tree.size();
        let scale = (512.0 / size.width().max(size.height())).min(1.0);
        let w = (size.width() * scale).ceil().max(1.0) as u32;
        let h = (size.height() * scale).ceil().max(1.0) as u32;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)
            .ok_or_else(|| io::Error::other("Invalid SVG dimensions"))?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        let mut rgba = pixmap.data().to_vec();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            // Un-premultiply: tiny-skia premultiplies, egui's textures want straight alpha. A
            // fully transparent pixel has no colour to recover, and its channels are already 0.
            let a = pixel[3] as u32;
            for c in &mut pixel[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a.max(1)).min(255) as u8;
            }
        }
        return image::RgbaImage::from_raw(w, h, rgba)
            .ok_or_else(|| io::Error::other("Invalid SVG"));
    }
    let image = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()
        .map_err(io::Error::other)?;
    Ok(if image.width() > 512 || image.height() > 512 {
        image.thumbnail(512, 512).to_rgba8()
    } else {
        image.to_rgba8()
    })
}
pub fn import(source: &Path) -> io::Result<String> {
    let decoded = decode(source)?;
    let bytes = std::fs::read(source)?;
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    let folder = writable_root().join("icons");
    std::fs::create_dir_all(&folder)?;
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    let ext = source.extension().unwrap_or_default().to_string_lossy();
    let base = format!("{stem}-{:016x}", hash.finish());
    let mut name = format!("{base}.{ext}");
    let mut suffix = 0;
    while folder.join(&name).exists() && std::fs::read(folder.join(&name))? != bytes {
        suffix += 1;
        name = format!("{base}-{suffix}.{ext}");
    }
    crate::util::write_atomic(&folder.join(&name), &bytes)?;
    if ext.eq_ignore_ascii_case("svg") {
        // Freeze external SVG image/font references into a portable rendered companion.
        let preview = format!("{name}.png");
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(decoded)
            .write_to(&mut png, image::ImageFormat::Png)
            .map_err(io::Error::other)?;
        crate::util::write_atomic(&folder.join(&preview), &png.into_inner())?;
        return Ok(format!("file:icons/{preview}"));
    }
    Ok(format!("file:icons/{name}"))
}
pub fn choose() -> std::io::Result<Option<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let mut buffer = vec![0u16; 32768];
    let filter: Vec<u16> = "Images and icons\0*.png;*.jpg;*.jpeg;*.bmp;*.gif;*.webp;*.tif;*.tiff;*.ico;*.svg;*.pnm;*.tga;*.dds;*.hdr;*.exr;*.qoi\0All files\0*.*\0\0".encode_utf16().collect();
    let title = super::wide("Choose a shortcut icon");
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner =
        unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow() };
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrTitle = title.as_ptr();
    dialog.lpstrFile = buffer.as_mut_ptr();
    dialog.nMaxFile = buffer.len() as u32;
    dialog.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    // SAFETY: dialog buffers remain alive until this modal call returns.
    if unsafe { GetOpenFileNameW(&mut dialog) } == 0 {
        let error = unsafe { CommDlgExtendedError() };
        return if error == 0 {
            Ok(None)
        } else {
            Err(std::io::Error::other(format!(
                "File picker error {error:#x}"
            )))
        };
    }
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Ok(Some(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..end],
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "nyx-images-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    #[test]
    fn common_image_formats_decode_and_preserve_proportions() {
        let root = temp();
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            32,
            16,
            image::Rgba([25, 75, 125, 255]),
        ));
        for ext in [
            "png", "jpg", "bmp", "gif", "webp", "tiff", "ico", "pnm", "tga", "qoi",
        ] {
            let path = root.join(format!("icon.{ext}"));
            if ext == "pnm" || ext == "jpg" {
                image.to_rgb8().save(&path).unwrap();
            } else {
                image.save(&path).unwrap();
            }
            let decoded = decode(&path).unwrap_or_else(|e| panic!("{ext}: {e}"));
            assert_eq!(decoded.dimensions(), (32, 16), "{ext}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn svg_and_png_keep_transparency() {
        let root = temp();
        let svg = root.join("icon.svg");
        std::fs::write(&svg, r##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16"><rect width="16" height="16" fill="#ff0000" fill-opacity="0.5"/></svg>"##).unwrap();
        let pixels = decode(&svg).unwrap();
        assert_eq!(pixels.dimensions(), (32, 16));
        assert_eq!(pixels.get_pixel(24, 8).0, [0, 0, 0, 0]);
        assert!(pixels.get_pixel(8, 8)[0] >= 250);
        assert!((127..=129).contains(&pixels.get_pixel(8, 8)[3]));
        let png = root.join("icon.png");
        pixels.save(&png).unwrap();
        assert_eq!(decode(&png).unwrap(), pixels);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn import_survives_original_deletion_and_rejects_corrupt_images() {
        let root = temp();
        let source = root.join("portable.png");
        image::RgbaImage::from_pixel(16, 16, image::Rgba([10, 20, 30, 255]))
            .save(&source)
            .unwrap();
        let spec = import(&source).unwrap();
        assert!(spec.starts_with("file:icons/"));
        assert_eq!(import(&source).unwrap(), spec);
        std::fs::remove_file(&source).unwrap();
        let copy = resolve(spec.strip_prefix("file:").unwrap());
        assert_eq!(decode(&copy).unwrap().dimensions(), (16, 16));
        std::fs::write(&source, b"not an image").unwrap();
        assert!(import(&source).is_err());
        std::fs::remove_file(copy).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
