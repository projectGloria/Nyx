//! Cell attributes resolved to concrete colours, exactly once, before anything reaches the GPU.
//!
//! This is a pure function of (cell, theme, palette) — it draws nothing and holds no state. It
//! lives under `render/` because it is the first half of drawing, and because keeping it in one
//! place is what lets `quads.rs` run-length merge backgrounds: "these two cells have the same
//! background" is then a comparison of resolved `Rgba`, not of flag combinations.

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};

use crate::theme::{Rgba, Theme};

/// What a cell actually looks like once every attribute has been applied.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Resolved {
    pub fg: Rgba,
    pub bg: Rgba,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
}

fn named(n: NamedColor, theme: &Theme, colors: &Colors) -> Rgba {
    if let Some(rgb) = colors[n] {
        return Rgba::from_rgb(rgb);
    }
    let i = n as usize;
    if i < 16 {
        return theme.palette[i];
    }
    match n {
        NamedColor::Foreground => theme.fg,
        NamedColor::Background => theme.bg,
        NamedColor::Cursor => theme.cursor_bg,
        NamedColor::BrightForeground => theme.fg,
        NamedColor::DimForeground => theme.fg.dim(),
        other => {
            // For a Dim<colour>, `to_bright` maps back to the base index, which is the only
            // thing we have a palette entry for.
            let base = other.to_bright() as usize;
            if base < 16 {
                theme.palette[base].dim()
            } else {
                theme.fg
            }
        },
    }
}

fn lookup(c: AnsiColor, theme: &Theme, colors: &Colors) -> Rgba {
    match c {
        AnsiColor::Spec(rgb) => Rgba::from_rgb(rgb),
        AnsiColor::Named(n) => named(n, theme, colors),
        AnsiColor::Indexed(i) => {
            let i = i as usize;
            colors[i].map(Rgba::from_rgb).unwrap_or(theme.palette[i.min(255)])
        },
    }
}

pub fn resolve(
    fg: AnsiColor,
    bg: AnsiColor,
    flags: Flags,
    theme: &Theme,
    colors: &Colors,
) -> Resolved {
    let bold = flags.contains(Flags::BOLD);

    // Bold promotes ANSI 0..8 to their bright twins. Every mainstream terminal does this, and
    // without it most shell prompts lose their colour entirely.
    let fg_color = if bold && theme.bold_is_bright {
        match fg {
            AnsiColor::Named(n) => AnsiColor::Named(n.to_bright()),
            AnsiColor::Indexed(i) if i < 8 => AnsiColor::Indexed(i + 8),
            other => other,
        }
    } else {
        fg
    };

    let mut resolved_fg = lookup(fg_color, theme, colors);
    let mut resolved_bg = lookup(bg, theme, colors);

    if flags.contains(Flags::DIM) {
        resolved_fg = resolved_fg.dim();
    }

    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut resolved_fg, &mut resolved_bg);
    }

    // HIDDEN is applied after INVERSE so that `\e[7;8m` hides against the swapped background
    // rather than against the original one.
    if flags.contains(Flags::HIDDEN) {
        resolved_fg = resolved_bg;
    }

    Resolved {
        fg: resolved_fg,
        bg: resolved_bg,
        bold,
        italic: flags.contains(Flags::ITALIC),
        underline: flags
            .intersects(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE | Flags::UNDERCURL),
        strikeout: flags.contains(Flags::STRIKEOUT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn inverse_swaps_fg_and_bg() {
        let t = theme();
        let c = Colors::default();
        let plain = resolve(
            AnsiColor::Named(NamedColor::Foreground),
            AnsiColor::Named(NamedColor::Background),
            Flags::empty(),
            &t,
            &c,
        );
        let inverted = resolve(
            AnsiColor::Named(NamedColor::Foreground),
            AnsiColor::Named(NamedColor::Background),
            Flags::INVERSE,
            &t,
            &c,
        );
        assert_eq!(plain.fg, inverted.bg);
        assert_eq!(plain.bg, inverted.fg);
    }

    #[test]
    fn hidden_makes_text_match_its_background_even_when_inverted() {
        let t = theme();
        let c = Colors::default();
        let r = resolve(
            AnsiColor::Named(NamedColor::Red),
            AnsiColor::Named(NamedColor::Background),
            Flags::INVERSE | Flags::HIDDEN,
            &t,
            &c,
        );
        assert_eq!(r.fg, r.bg);
    }

    #[test]
    fn bold_promotes_dull_ansi_to_bright() {
        let mut t = theme();
        t.bold_is_bright = true;
        let c = Colors::default();
        let plain =
            resolve(AnsiColor::Indexed(1), AnsiColor::Indexed(0), Flags::empty(), &t, &c);
        let bold = resolve(AnsiColor::Indexed(1), AnsiColor::Indexed(0), Flags::BOLD, &t, &c);
        assert_eq!(plain.fg, t.palette[1]);
        assert_eq!(bold.fg, t.palette[9]);
        // ...but only the low 8. Index 9 is already bright and must not run off the end.
        let already_bright =
            resolve(AnsiColor::Indexed(9), AnsiColor::Indexed(0), Flags::BOLD, &t, &c);
        assert_eq!(already_bright.fg, t.palette[9]);
    }

    #[test]
    fn an_osc_set_palette_entry_wins_over_the_theme() {
        let t = theme();
        let mut c = Colors::default();
        let custom = alacritty_terminal::vte::ansi::Rgb { r: 1, g: 2, b: 3 };
        c[1] = Some(custom);
        let r = resolve(AnsiColor::Indexed(1), AnsiColor::Indexed(0), Flags::empty(), &t, &c);
        assert_eq!(r.fg, Rgba::rgb(1, 2, 3));
    }
}
