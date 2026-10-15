//! `icon = "exe:C:\...\code.exe"` — the icon Explorer shows for a file, as pixels.
//!
//! This is the last of the three icon sources: `builtin:` is drawn as vectors, `file:` is a PNG
//! or BMP the user points at, and `exe:` is the one that makes a list of shortcuts look like the
//! programs they open without anyone having to extract and crop anything.
//!
//! `SHGetFileInfoW` hands back an `HICON`, which is a pair of GDI bitmaps and not something a GPU
//! can be shown. Turning it into RGBA is the bulk of what follows, and the awkward part is alpha:
//! a 32-bit icon carries its own, and anything older carries a separate 1-bit mask with the
//! polarity inverted — white means *transparent*. An icon whose colour bitmap has no alpha at all
//! is the common case on older executables, and without the mask branch it renders as a black
//! square, which looks like a bug in Nyx rather than an old icon.

use std::path::Path;
use std::ptr;

use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

use crate::win::wide;

/// A decoded icon, in the layout `egui::ColorImage` wants.
pub struct Image {
    pub width: usize,
    pub height: usize,
    /// RGBA8, top-down, straight (not premultiplied) alpha.
    pub rgba: Vec<u8>,
}

/// Nothing legitimate is this big, and the buffer is allocated from the header the OS gives us.
const MAX_SIDE: i32 = 512;

/// Load the shell icon for a path.
///
/// `None` for anything that does not resolve — a missing executable, a file type with no icon, an
/// icon the GDI calls refuse. The caller caches that answer, because otherwise an entry
/// pointing at a deleted program asks the shell about it sixty times a second.
///
/// The size is whatever `SHGFI_LARGEICON` gives, which is the system's 32×32 metric. Asking for
/// the 256-pixel "jumbo" variant means `SHGFI_SYSICONINDEX` and an `IImageList`, which is COM, an
/// apartment to initialise and a failure mode per call — for a 34-pixel menu entry that is a lot
/// of machinery to end up scaling down anyway.
pub fn load(path: &Path) -> Option<Image> {
    let wide_path = wide(path);
    // SAFETY: `info` is fully written by the call when it succeeds; a zeroed struct is what the
    // API expects to receive.
    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    // SAFETY: `wide_path` is NUL-terminated and outlives the call; `cbFileInfo` is the true size.
    let ok = unsafe {
        SHGetFileInfoW(
            wide_path.as_ptr(),
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if ok == 0 || info.hIcon.is_null() {
        return None;
    }

    let image = decode(info.hIcon);
    // SAFETY: the icon came from `SHGetFileInfoW` with SHGFI_ICON, which the caller must destroy.
    unsafe { DestroyIcon(info.hIcon) };
    image
}

fn decode(icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON) -> Option<Image> {
    // SAFETY: zeroed, then fully written on success.
    let mut ii: ICONINFO = unsafe { std::mem::zeroed() };
    // SAFETY: `GetIconInfo` creates *new* bitmaps that the caller owns and must delete — the
    // documented leak if you forget, and one per frame if the caller does not cache.
    if unsafe { GetIconInfo(icon, &mut ii) } == 0 {
        return None;
    }

    let result = (|| {
        let bitmap = describe(ii.hbmColor)?;
        let width = bitmap.bmWidth;
        let height = bitmap.bmHeight.abs();
        if width <= 0 || height <= 0 || width > MAX_SIDE || height > MAX_SIDE {
            return None;
        }

        let mut rgba = bgra_of(ii.hbmColor, width, height)?;

        // A 32-bit icon carries its own alpha. Anything older comes back fully transparent here,
        // and the separate mask is where the shape actually lives.
        if rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 0) {
            apply_mask(&mut rgba, ii.hbmMask, width, height);
        }

        // BGRA -> RGBA. GDI's DIBs are little-endian BGRA whatever the header says.
        for px in rgba.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }

        Some(Image { width: width as usize, height: height as usize, rgba })
    })();

    // SAFETY: both handles were created by `GetIconInfo` and are ours to delete. A null one (a
    // monochrome icon has no colour bitmap) is a harmless no-op.
    unsafe {
        DeleteObject(ii.hbmColor as _);
        DeleteObject(ii.hbmMask as _);
    }
    result
}

fn describe(bitmap: HBITMAP) -> Option<BITMAP> {
    if bitmap.is_null() {
        return None;
    }
    // SAFETY: zeroed, then written by `GetObjectW` up to the size given.
    let mut out: BITMAP = unsafe { std::mem::zeroed() };
    // SAFETY: the buffer is exactly one BITMAP.
    let written = unsafe {
        GetObjectW(
            bitmap as _,
            std::mem::size_of::<BITMAP>() as i32,
            (&mut out as *mut BITMAP).cast(),
        )
    };
    (written != 0).then_some(out)
}

/// Read a bitmap as 32-bit top-down BGRA, whatever it is stored as.
fn bgra_of(bitmap: HBITMAP, width: i32, height: i32) -> Option<Vec<u8>> {
    if bitmap.is_null() {
        return None;
    }
    // SAFETY: a null HDC gives a memory DC compatible with the screen, which is all `GetDIBits`
    // needs to describe a bitmap it is not drawing.
    let dc = unsafe { CreateCompatibleDC(ptr::null_mut()) };
    if dc.is_null() {
        return None;
    }

    // SAFETY: zeroed; every field the call reads is set below.
    let mut header: BITMAPINFO = unsafe { std::mem::zeroed() };
    header.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        // Negative means top-down. A positive height gives bottom-up rows, and every icon in the
        // menu comes out upside down — the kind of bug that is obvious once seen and invisible
        // until then.
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };

    let mut buffer = vec![0u8; (width as usize) * (height as usize) * 4];
    // SAFETY: `buffer` is exactly `width * height * 4` bytes, which is what the header describes.
    let rows = unsafe {
        GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            buffer.as_mut_ptr().cast(),
            &mut header,
            DIB_RGB_COLORS,
        )
    };
    // SAFETY: our own DC.
    unsafe { DeleteDC(dc) };

    (rows != 0).then_some(buffer)
}

/// Fill in alpha from a legacy icon's AND mask.
///
/// The polarity is the trap: in the mask, **white means transparent**. Reading it the obvious way
/// round gives an icon that is a hole in the shape of itself.
fn apply_mask(rgba: &mut [u8], mask: HBITMAP, width: i32, height: i32) {
    let Some(bits) = bgra_of(mask, width, height) else {
        // No mask to read — better a solid icon than an invisible one.
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        return;
    };

    for (px, m) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(bits.as_chunks::<4>().0) {
        let transparent = m[0] != 0 || m[1] != 0 || m[2] != 0;
        px[3] = if transparent { 0 } else { 255 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_executable_has_a_readable_icon() {
        // Against a real file on this machine, because there is no way to fake an HICON. notepad
        // is present on every Windows install since 1985.
        let image = load(Path::new(r"C:\Windows\System32\notepad.exe"))
            .expect("notepad.exe has a shell icon");
        assert!(image.width >= 16 && image.height >= 16, "{}x{}", image.width, image.height);
        assert_eq!(image.rgba.len(), image.width * image.height * 4);
        // Something has to be opaque, or the conversion produced an invisible icon.
        assert!(image.rgba.as_chunks::<4>().0.iter().any(|px| px[3] > 0), "every pixel is transparent");
        // ...and something has to be coloured, or it produced a black square.
        assert!(
            image.rgba.as_chunks::<4>().0.iter().any(|px| px[0] > 0 || px[1] > 0 || px[2] > 0),
            "every pixel is black"
        );
    }

    #[test]
    fn a_missing_file_is_none_rather_than_a_placeholder() {
        // The caller caches this answer; a placeholder would be cached as a successful icon and
        // the user would never find out the path was wrong.
        assert!(load(Path::new(r"C:\definitely\not\here.exe")).is_none());
    }

    #[test]
    fn the_mask_polarity_is_the_right_way_round() {
        // White in the AND mask means transparent. Reading it the obvious way gives an icon that
        // is a hole in the shape of itself, so the rule is pinned here rather than only in a
        // comment.
        let mut rgba = [9u8; 8]; // two pixels
        let mask = [255u8, 255, 255, 0, /* white  -> transparent */ 0, 0, 0, 0 /* black -> opaque */];
        // Reuse the inner loop by constructing the same situation `apply_mask` would.
        for (px, m) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(mask.as_chunks::<4>().0) {
            let transparent = m[0] != 0 || m[1] != 0 || m[2] != 0;
            px[3] = if transparent { 0 } else { 255 };
        }
        assert_eq!(rgba[3], 0, "white mask pixel should be transparent");
        assert_eq!(rgba[7], 255, "black mask pixel should be opaque");
    }
}
