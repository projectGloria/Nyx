//! Shortcut icons.
//!
//! `builtin:` icons are drawn as vectors rather than loaded as images: they scale to any DPI
//! without a second asset, they take their colour from the theme (or the shortcut's own), and
//! they add no dependency. `file:` icons are decoded from disk by `win::assets` — raster formats
//! through the `image` crate, SVG through resvg — which is what makes them genuinely
//! customizable.
//!
//! `exe:` lifts the icon out of an executable through the shell, which is what makes a list of
//! shortcuts look like the programs they open without anyone having to find and crop a PNG. It is
//! the only one of the three that costs a syscall, so its results are cached — see [`Cache`].

use egui::{Color32, Pos2, Rect, Stroke, Vec2};

/// Which icon a shortcut asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Icon {
    Builtin(Builtin),
    File(String),
    /// `exe:C:\...\code.exe` — whatever Explorer shows for that file.
    Exe(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Folder,
    Terminal,
    Code,
    Home,
    Star,
    Gear,
    Download,
    Disk,
}

impl Icon {
    /// Parse `builtin:folder` / `file:C:\x.png`. An unrecognised spec becomes a folder rather
    /// than nothing, so a typo still leaves a clickable entry.
    pub fn parse(spec: &str) -> Self {
        let spec = spec.trim();
        if let Some(name) = spec.strip_prefix("builtin:") {
            return Icon::Builtin(Builtin::parse(name));
        }
        if let Some(path) = spec.strip_prefix("file:") {
            return Icon::File(path.trim().to_string());
        }
        if let Some(path) = spec.strip_prefix("exe:") {
            return Icon::Exe(path.trim().to_string());
        }
        // Bare names are treated as builtins so `icon = "star"` does the obvious thing.
        Icon::Builtin(Builtin::parse(spec))
    }
}

/// Executable icons that have already been asked for.
///
/// `SHGetFileInfoW` reads the file, and an entry drawn every frame would ask the shell about
/// the same `.exe` sixty times a second. A failure is cached too, as `None`: a shortcut pointing
/// at a program that has been uninstalled must stop asking, not keep trying.
#[derive(Default)]
pub struct Cache {
    /// Two maps rather than one keyed by a prefixed string. The lookup happens once per drawn
    /// entry per frame, and building `format!("exe:{path}")` to ask a question makes a heap
    /// allocation out of a cache *hit* — which is the case this type exists to make free.
    files: rustc_hash::FxHashMap<String, Option<egui::TextureHandle>>,
    exes: rustc_hash::FxHashMap<String, Option<egui::TextureHandle>>,
}

impl Cache {
    /// An image file from disk, for `icon = "file:..."`.
    ///
    /// Decoded here rather than handed to egui as a `file://` URI. egui resolves URIs through
    /// registered loaders and Nyx registers none — `egui_extras::install_image_loaders` is what
    /// supplies the file and image ones — so every `file:` icon rendered as the error placeholder,
    /// including the ones pointing at a perfectly good PNG. Loading it here also puts it on the
    /// same path as `exe:`: cached by spec, failures cached as `None`, and a folder glyph rather
    /// than an error square when the file is missing.
    ///
    /// Raster images and SVG use the same decoder as the upload validator.
    pub fn file(&mut self, ctx: &egui::Context, path: &str) -> Option<&egui::TextureHandle> {
        if !self.files.contains_key(path) {
            let loaded = (|| {
                let rgba = crate::win::assets::decode(&crate::win::assets::resolve(path)).ok()?;
                let size = [rgba.width() as usize, rgba.height() as usize];
                // PNG carries straight alpha, not premultiplied.
                let colour = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                Some(ctx.load_texture(
                    format!("nyx.icon.file.{path}"),
                    colour,
                    egui::TextureOptions::LINEAR,
                ))
            })();
            self.files.insert(path.to_owned(), loaded);
        }
        self.files.get(path).and_then(Option::as_ref)
    }

    pub fn exe(&mut self, ctx: &egui::Context, path: &str) -> Option<&egui::TextureHandle> {
        if !self.exes.contains_key(path) {
            let loaded = crate::win::shellicon::load(std::path::Path::new(path)).map(|image| {
                // Straight alpha, not premultiplied: `shellicon` reconstructs it from the icon's
                // own channel or from the legacy mask, and neither has been multiplied through.
                let colour = egui::ColorImage::from_rgba_unmultiplied(
                    [image.width, image.height],
                    &image.rgba,
                );
                ctx.load_texture(
                    format!("nyx.icon.{path}"),
                    colour,
                    // A 32-pixel shell icon drawn into a tab or menu row is almost always scaled;
                    // nearest-neighbour would make that obvious in the worst way.
                    egui::TextureOptions::LINEAR,
                )
            });
            self.exes.insert(path.to_owned(), loaded);
        }
        self.exes.get(path).and_then(Option::as_ref)
    }

    /// Forget everything, so a reopened settings window re-reads an icon file replaced on disk.
    ///
    /// Tied to opening the window on purpose: that is a user action, it happens once, and it is
    /// the only moment at which the *contents* behind an unchanged path plausibly changed. See
    /// [`Cache::retain`] for why the app-wide cache must not do this.
    pub fn clear(&mut self) {
        self.files.clear();
        self.exes.clear();
    }

    /// Drop the entries no shortcut asks for any more.
    ///
    /// Not a `clear`. Both maps are keyed by path, so a shortcut that now points somewhere else is
    /// already a miss and needs no invalidation — and clearing ran on every settings keystroke,
    /// because the window applies its draft as you type. That cost a `SHGetFileInfoW` or a decode
    /// and a texture upload per drawn icon per character typed, which is the defect the shellgen
    /// worker exists to avoid, arriving by another route.
    ///
    /// Retaining rather than doing nothing at all is what keeps the map bounded: typing a path
    /// into the icon field inserts a failed lookup per prefix, and those would otherwise stay for
    /// the life of the process.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.files.retain(|path, _| keep(path));
        self.exes.retain(|path, _| keep(path));
    }
}

impl Builtin {
    /// Every builtin, in the order the settings GUI offers them.
    pub const ALL: [Builtin; 8] = [
        Builtin::Folder,
        Builtin::Terminal,
        Builtin::Code,
        Builtin::Home,
        Builtin::Star,
        Builtin::Gear,
        Builtin::Download,
        Builtin::Disk,
    ];

    /// The name this icon is written as in `config.toml`.
    pub fn name(self) -> &'static str {
        match self {
            Builtin::Folder => "folder",
            Builtin::Terminal => "terminal",
            Builtin::Code => "code",
            Builtin::Home => "home",
            Builtin::Star => "star",
            Builtin::Gear => "gear",
            Builtin::Download => "download",
            Builtin::Disk => "disk",
        }
    }

    fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "terminal" | "shell" | "console" => Builtin::Terminal,
            "code" | "dev" | "source" => Builtin::Code,
            "home" => Builtin::Home,
            "star" | "favourite" | "favorite" => Builtin::Star,
            "gear" | "settings" | "config" => Builtin::Gear,
            "download" | "downloads" => Builtin::Download,
            "disk" | "drive" => Builtin::Disk,
            _ => Builtin::Folder,
        }
    }
}

/// Draw a builtin icon inside `rect`.
pub fn draw_builtin(painter: &egui::Painter, rect: Rect, icon: Builtin, color: Color32) {
    let r = rect.shrink(rect.width() * 0.16);
    let stroke = Stroke::new((r.width() * 0.09).max(1.2), color);

    match icon {
        Builtin::Folder => {
            // A tab along the top-left, then the body.
            let body = Rect::from_min_max(
                Pos2::new(r.min.x, r.min.y + r.height() * 0.22),
                Pos2::new(r.max.x, r.max.y),
            );
            painter.rect_stroke(body, r.width() * 0.10, stroke, egui::StrokeKind::Inside);
            painter.line_segment(
                [
                    Pos2::new(r.min.x + r.width() * 0.06, r.min.y + r.height() * 0.22),
                    Pos2::new(r.min.x + r.width() * 0.40, r.min.y + r.height() * 0.22),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(r.min.x + r.width() * 0.06, r.min.y + r.height() * 0.22),
                    Pos2::new(r.min.x + r.width() * 0.16, r.min.y + r.height() * 0.06),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(r.min.x + r.width() * 0.16, r.min.y + r.height() * 0.06),
                    Pos2::new(r.min.x + r.width() * 0.46, r.min.y + r.height() * 0.06),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(r.min.x + r.width() * 0.46, r.min.y + r.height() * 0.06),
                    Pos2::new(r.min.x + r.width() * 0.52, r.min.y + r.height() * 0.22),
                ],
                stroke,
            );
        }
        Builtin::Terminal => {
            painter.rect_stroke(r, r.width() * 0.12, stroke, egui::StrokeKind::Inside);
            // A chevron and an underscore: the universal shorthand for a prompt.
            let c = r.center();
            let s = r.width() * 0.16;
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 1.6, c.y - s),
                    Pos2::new(c.x - s * 0.4, c.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.4, c.y),
                    Pos2::new(c.x - s * 1.6, c.y + s),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x + s * 0.2, c.y + s),
                    Pos2::new(c.x + s * 1.6, c.y + s),
                ],
                stroke,
            );
        }
        Builtin::Code => {
            let c = r.center();
            let s = r.width() * 0.22;
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.4, c.y - s),
                    Pos2::new(c.x - s * 1.5, c.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 1.5, c.y),
                    Pos2::new(c.x - s * 0.4, c.y + s),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x + s * 0.4, c.y - s),
                    Pos2::new(c.x + s * 1.5, c.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x + s * 1.5, c.y),
                    Pos2::new(c.x + s * 0.4, c.y + s),
                ],
                stroke,
            );
        }
        Builtin::Home => {
            let c = r.center();
            let w = r.width() * 0.38;
            painter.line_segment(
                [Pos2::new(c.x - w, c.y), Pos2::new(c.x, c.y - w * 0.85)],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(c.x, c.y - w * 0.85), Pos2::new(c.x + w, c.y)],
                stroke,
            );
            let body = Rect::from_min_max(
                Pos2::new(c.x - w * 0.72, c.y),
                Pos2::new(c.x + w * 0.72, c.y + w * 0.8),
            );
            painter.rect_stroke(body, 1.0, stroke, egui::StrokeKind::Inside);
        }
        Builtin::Star => {
            let c = r.center();
            let outer = r.width() * 0.42;
            let inner = outer * 0.42;
            let mut points = Vec::with_capacity(10);
            for i in 0..10 {
                let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                let radius = if i % 2 == 0 { outer } else { inner };
                points.push(Pos2::new(
                    c.x + angle.cos() * radius,
                    c.y + angle.sin() * radius,
                ));
            }
            painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        }
        Builtin::Gear => {
            let c = r.center();
            painter.circle_stroke(c, r.width() * 0.26, stroke);
            for i in 0..6 {
                let angle = i as f32 * std::f32::consts::PI / 3.0;
                let v = Vec2::new(angle.cos(), angle.sin());
                painter.line_segment([c + v * r.width() * 0.30, c + v * r.width() * 0.45], stroke);
            }
        }
        Builtin::Download => {
            let c = r.center();
            let h = r.height() * 0.32;
            painter.line_segment(
                [Pos2::new(c.x, c.y - h), Pos2::new(c.x, c.y + h * 0.5)],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(c.x - h * 0.5, c.y), Pos2::new(c.x, c.y + h * 0.5)],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(c.x + h * 0.5, c.y), Pos2::new(c.x, c.y + h * 0.5)],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(c.x - h, c.y + h), Pos2::new(c.x + h, c.y + h)],
                stroke,
            );
        }
        Builtin::Disk => {
            painter.rect_stroke(r, r.width() * 0.12, stroke, egui::StrokeKind::Inside);
            painter.circle_stroke(r.center(), r.width() * 0.16, stroke);
        }
    }
}

/// Center an image in the icon cell without distorting its proportions.
pub fn fitted_rect(rect: egui::Rect, texture: &egui::TextureHandle) -> egui::Rect {
    let size = texture.size_vec2();
    let scale = (rect.width() / size.x).min(rect.height() / size.y);
    egui::Rect::from_center_size(rect.center(), size * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_specs_parse() {
        assert_eq!(
            Icon::parse("builtin:terminal"),
            Icon::Builtin(Builtin::Terminal)
        );
        assert_eq!(Icon::parse("  builtin:Star "), Icon::Builtin(Builtin::Star));
        assert_eq!(
            Icon::parse("file:C:\\a.png"),
            Icon::File("C:\\a.png".into())
        );
        assert_eq!(
            Icon::parse(r"exe:C:\Windows\System32\notepad.exe"),
            Icon::Exe(r"C:\Windows\System32\notepad.exe".into())
        );
        // A bare name is a builtin, so `icon = "gear"` does the obvious thing.
        assert_eq!(Icon::parse("gear"), Icon::Builtin(Builtin::Gear));
    }

    #[test]
    fn a_drive_letter_is_not_mistaken_for_a_scheme() {
        // `C:` looks like a prefix and is not one. Only the three known schemes are stripped, and
        // anything else falls through to the builtin table rather than losing its drive letter.
        assert_eq!(Icon::parse(r"C:\x.png"), Icon::Builtin(Builtin::Folder));
    }

    #[test]
    fn an_unknown_icon_falls_back_to_a_folder_rather_than_nothing() {
        // A typo should still leave a clickable entry.
        assert_eq!(Icon::parse("builtin:wat"), Icon::Builtin(Builtin::Folder));
        assert_eq!(Icon::parse(""), Icon::Builtin(Builtin::Folder));
    }

    #[test]
    fn aliases_for_the_same_icon_agree() {
        assert_eq!(Icon::parse("shell"), Icon::parse("terminal"));
        assert_eq!(Icon::parse("favourite"), Icon::parse("favorite"));
        assert_eq!(Icon::parse("settings"), Icon::parse("gear"));
    }

    #[test]
    fn every_listed_icon_parses_back_to_itself() {
        // The settings GUI writes `builtin:<name>` from this list. A name that does not parse
        // would silently become a folder the next time the config was read.
        for icon in Builtin::ALL {
            assert_eq!(
                Icon::parse(&format!("builtin:{}", icon.name())),
                Icon::Builtin(icon),
                "{} did not round-trip",
                icon.name()
            );
        }
    }
}
