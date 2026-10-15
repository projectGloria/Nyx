//! Which shells exist, and the `[[profile]]` entries that adjust them.
//!
//! Two lists, because the config format has two ideas in it. The top list is what Nyx *found* and
//! will offer; the bottom list is the overrides in `config.toml`. Showing only the second would
//! hide the reason a shell is missing, and showing only the first would make the file look like
//! something Nyx wrote rather than something the user owns.

use super::widgets;
use super::State;
use crate::config::schema::ProfileConfig;
use crate::term::detect::Detected;
use crate::term::profile::{Profile, ProfileKind};
use crate::theme::visuals::to_color32;
use crate::theme::Theme;

const KINDS: [ProfileKind; 4] =
    [ProfileKind::Cmd, ProfileKind::PowerShell, ProfileKind::Bash, ProfileKind::Wsl];

pub fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    profiles: &[Profile],
    detected: &Detected,
    theme: &Theme,
) -> bool {
    let mut changed = false;

    ui.label(egui::RichText::new("Found on this machine").strong());
    ui.add_space(4.0);

    if profiles.is_empty() {
        ui.colored_label(to_color32(theme.palette[1]), "no shells detected");
    }

    let mut customise: Option<String> = None;
    egui::Grid::new("nyx.settings.detected")
        .num_columns(4)
        .spacing([14.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            for profile in profiles {
                ui.label(&profile.name);
                ui.label(
                    egui::RichText::new(profile.program.to_string_lossy().into_owned())
                        .monospace()
                        .small(),
                );

                // The *effective* answer, from the same function `launch` uses, not the flag.
                let injects = profile.injects(detected.clink.as_deref());
                let (text, color) = if injects {
                    ("shell-side expansion", theme.palette[2])
                } else {
                    ("terminal-side fallback", theme.palette[8])
                };
                ui.label(egui::RichText::new(text).small().color(to_color32(color)));

                let overridden = state.draft.profiles.iter().any(|p| p.id == profile.id);
                if ui
                    .add_enabled(!overridden, egui::Button::new("Customise"))
                    .on_hover_text("add a [[profile]] entry for this shell")
                    .clicked()
                {
                    customise = Some(profile.id.clone());
                }
                ui.end_row();
            }
        });

    if let Some(id) = customise {
        state.draft.profiles.push(ProfileConfig { id, ..Default::default() });
        changed = true;
    }

    ui.add_space(6.0);
    clink_status(ui, detected, theme);

    ui.add_space(14.0);
    ui.separator();
    ui.label(egui::RichText::new("Your profiles").strong());
    ui.label(
        egui::RichText::new(
            "An entry matching a detected id changes only the fields you fill in, so adding an \
             argument does not mean restating the path.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    let mut remove: Option<usize> = None;
    for (i, profile) in state.draft.profiles.iter_mut().enumerate() {
        let title = if profile.name.trim().is_empty() {
            profile.id.clone()
        } else {
            format!("{} ({})", profile.name, profile.id)
        };
        egui::CollapsingHeader::new(if title.trim().is_empty() {
            format!("profile {}", i + 1)
        } else {
            title
        })
        .id_salt(("nyx.settings.profile", i))
        .default_open(true)
        .show(ui, |ui| {
            changed |= fields(ui, i, profile);
            if ui.button("Remove").clicked() {
                remove = Some(i);
            }
        });
    }

    if ui.button("Add a profile").clicked() {
        state.draft.profiles.push(ProfileConfig {
            id: unique_id(&state.draft.profiles),
            ..Default::default()
        });
        changed = true;
    }
    if let Some(i) = remove {
        state.draft.profiles.remove(i);
        changed = true;
    }

    changed
}

fn fields(ui: &mut egui::Ui, index: usize, profile: &mut ProfileConfig) -> bool {
    let mut changed = false;

    egui::Grid::new(("nyx.settings.profile.grid", index))
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            changed |= widgets::text(ui, "Id", &mut profile.id, 200.0);
            changed |= widgets::text(ui, "Name", &mut profile.name, 240.0);
            changed |= widgets::text(ui, "Program", &mut profile.program, 380.0);

            ui.label("Arguments");
            ui.horizontal(|ui| {
                // Absent and empty are different: absent inherits the detected arguments, empty
                // deliberately launches the shell bare. A single text field cannot say both, so
                // the checkbox is what distinguishes them.
                let mut has = profile.args.is_some();
                if ui.checkbox(&mut has, "override").changed() {
                    profile.args = has.then(Vec::new);
                    changed = true;
                }
                match profile.args.as_mut() {
                    Some(args) => {
                        // The buffer lives across frames. Rebuilding it from `args.join(" ")`
                        // every frame and re-splitting it immediately meant a trailing space was
                        // discarded before it could become a separator — so the space bar did
                        // nothing and a second argument could never be typed at all.
                        let id = ui.make_persistent_id(("nyx.settings.args", index));
                        let mut line = ui
                            .data_mut(|d| d.get_temp::<String>(id))
                            .unwrap_or_else(|| join_args(args));

                        let response = ui.add(
                            egui::TextEdit::singleline(&mut line)
                                .desired_width(320.0)
                                .font(egui::TextStyle::Monospace),
                        );

                        if response.changed() {
                            *args = split_args(&line);
                            changed = true;
                        }
                        if response.lost_focus() {
                            // Drop the buffer so the field re-reads from what was actually
                            // stored — which is also how a value changed elsewhere shows up.
                            ui.data_mut(|d| d.remove::<String>(id));
                        } else {
                            ui.data_mut(|d| d.insert_temp(id, line));
                        }
                    },
                    None => {
                        ui.weak("using the detected arguments");
                    },
                }
            });
            ui.end_row();

            ui.label("Kind");
            ui.horizontal(|ui| {
                let label = if profile.kind.trim().is_empty() {
                    "from the program name".to_string()
                } else {
                    profile.kind.clone()
                };
                egui::ComboBox::from_id_salt(("nyx.settings.kind", index))
                    .width(190.0)
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                profile.kind.trim().is_empty(),
                                "from the program name",
                            )
                            .clicked()
                        {
                            profile.kind.clear();
                            changed = true;
                        }
                        for kind in KINDS {
                            if ui
                                .selectable_label(profile.kind == kind.name(), kind.name())
                                .clicked()
                            {
                                profile.kind = kind.name().to_string();
                                changed = true;
                            }
                        }
                    });
                ui.weak("decides how @shortcut paths are spelled");
            });
            ui.end_row();

            changed |= widgets::text(ui, "Start in", &mut profile.cwd, 380.0);

            ui.label("Shell integration");
            ui.horizontal(|ui| {
                let mut auto = !profile.integration.trim().eq_ignore_ascii_case("none");
                if ui.checkbox(&mut auto, "inject the init script").changed() {
                    profile.integration = if auto { "auto".into() } else { "none".into() };
                    changed = true;
                }
                ui.weak("off falls back to terminal-side expansion");
            });
            ui.end_row();

            changed |= widgets::toggle(
                ui,
                "Enabled",
                &mut profile.enabled,
                "off hides this shell from the palette and the + menu",
            );
        });

    changed
}

fn clink_status(ui: &mut egui::Ui, detected: &Detected, theme: &Theme) {
    match &detected.clink {
        Some(path) => {
            ui.label(
                egui::RichText::new(format!(
                    "Clink found at {} \u{2014} cmd.exe expands in its line editor.",
                    path.display()
                ))
                .small()
                .color(to_color32(theme.palette[2])),
            );
        },
        None => {
            ui.label(
                egui::RichText::new(
                    "Clink is not installed, so cmd.exe uses the terminal-side fallback: \
                     shortcuts still expand, just at the moment you press space rather than in \
                     the line editor.",
                )
                .small()
                .weak(),
            );
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("To fix:").small().weak());
                let mut command = "winget install chrisant996.clink".to_string();
                ui.add(
                    egui::TextEdit::singleline(&mut command)
                        .desired_width(300.0)
                        .font(egui::TextStyle::Monospace),
                );
            });
        },
    }
}

fn unique_id(existing: &[ProfileConfig]) -> String {
    let taken = |id: &str| existing.iter().any(|p| p.id == id);
    if !taken("profile") {
        return "profile".into();
    }
    (2..).map(|n| format!("profile{n}")).find(|id| !taken(id)).unwrap_or_else(|| "profile".into())
}

/// Split an argument line the way a shell would.
///
/// `split_whitespace` was not that: it broke `-c "hello world"` into three arguments, each half of
/// the quoted string carrying a stray quote into the command line of a real process.
///
/// Quotes group and are not themselves part of the argument. Backslash escaping is deliberately
/// not supported — a Windows argument is full of backslashes and treating them as escapes would
/// mangle every path anyone typed here.
fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;

    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                // An empty quoted string is a real, empty argument.
                started = true;
            },
            None if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            },
            None => {
                current.push(c);
                started = true;
            },
        }
    }
    if started {
        out.push(current);
    }
    out
}

/// The inverse: what to put in the editor for a stored argument list.
///
/// Everything the splitter would group is quoted here, so any list this produced reads back as
/// itself. That includes arguments carrying a *quote character* and no whitespace — the splitter
/// opens a quote on the first `'` or `"` it meets, so an unquoted `it's` came back as `its`. The
/// quote kind is whichever the argument itself does not contain. An argument carrying both kinds
/// cannot be spelled here without escape rules the splitter deliberately has not; it is
/// double-quoted as the best effort available, and the next edit shows what was stored.
fn join_args(args: &[String]) -> String {
    args.iter().map(|a| join_arg(a)).collect::<Vec<_>>().join(" ")
}

fn join_arg(a: &str) -> String {
    let needs_quoting = a.is_empty()
        || a.chars().any(|c| c.is_whitespace())
        || a.contains('"')
        || a.contains('\'');
    if !needs_quoting {
        return a.to_string();
    }
    if a.contains('"') && !a.contains('\'') {
        format!("'{a}'")
    } else {
        format!("\"{a}\"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailing_space_is_a_separator_rather_than_a_deletion() {
        // The buffer keeps what was typed; this is only what it means so far. Re-rendering the
        // field from this value every frame is what ate the space and made the space bar look
        // broken in the one field where it is the separator.
        assert_eq!(split_args("-NoProfile "), vec!["-NoProfile"]);
        assert_eq!(split_args("-NoProfile -x"), vec!["-NoProfile", "-x"]);
        assert_eq!(split_args("   "), Vec::<String>::new());
        assert_eq!(split_args(""), Vec::<String>::new());
    }

    #[test]
    fn quotes_group_arguments_that_contain_spaces() {
        assert_eq!(split_args(r#"-c "hello world""#), vec!["-c", "hello world"]);
        assert_eq!(split_args("-c 'hello world'"), vec!["-c", "hello world"]);
        // A quote inside the other kind is an ordinary character.
        assert_eq!(split_args(r#""it's fine""#), vec!["it's fine"]);
        // An empty quoted string is an argument, not nothing.
        assert_eq!(split_args(r#"-c "" -x"#), vec!["-c", "", "-x"]);
    }

    #[test]
    fn backslashes_are_left_alone_because_they_are_paths_here() {
        assert_eq!(
            split_args(r"-file C:\Users\Nihil\x.ps1"),
            vec!["-file", r"C:\Users\Nihil\x.ps1"]
        );
    }

    #[test]
    fn an_argument_list_survives_the_round_trip_through_the_editor() {
        // What the field shows has to parse back to what was stored, or opening the settings
        // window and closing it again would silently rewrite the profile.
        for args in [
            vec![],
            vec!["-NoProfile".to_string()],
            vec!["-NoProfile".to_string(), "-NoExit".to_string()],
            vec!["-c".to_string(), "hello world".to_string()],
            vec!["-c".to_string(), String::new()],
            vec!["--login".to_string(), r"C:\Program Files\x".to_string()],
            // An argument carrying a quote character used to come back *mangled*, not merely
            // re-quoted: the bare `it's` opened a single-quote group on the `'` and the splitter
            // handed back `its`.
            vec!["it's".to_string()],
            vec![r#"say "hi""#.to_string()],
            vec![r#"-Command "Get-ChildItem""#.to_string()],
        ] {
            assert_eq!(split_args(&join_args(&args)), args, "{args:?} via {:?}", join_args(&args));
        }
    }
}
