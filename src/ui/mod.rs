//! Chrome: everything that is not the terminal grid.

pub mod complete;
pub mod findbar;
pub mod copyblock;
pub mod icons;
pub mod palette;
pub mod window;
pub mod settings;

/// Keep keyboard selection visible when a menu has more results than painted rows.
pub fn selection_window(selected: usize, len: usize, limit: usize) -> std::ops::Range<usize> {
    let count = len.min(limit);
    let start = selected.saturating_sub(count.saturating_sub(1)).min(len - count);
    start..start + count
}

/// Both painted input boxes consume text and editing keys in their arrival order.
pub fn edit_query(query: &mut String, event: &egui::Event) -> bool {
    match event {
        egui::Event::Text(text) | egui::Event::Paste(text)
            | egui::Event::Ime(egui::ImeEvent::Commit(text)) => query.push_str(text),
        egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => { query.pop(); },
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn query_edits_follow_text_key_paste_and_ime_order() {
        let mut query = String::new();
        let events = [egui::Event::Text("ab".into()), egui::Event::Key {
            key: egui::Key::Backspace, physical_key: None, pressed: true,
            repeat: false, modifiers: egui::Modifiers::NONE,
        }, egui::Event::Paste("世界".into()), egui::Event::Ime(egui::ImeEvent::Commit("é".into()))];
        for event in &events { assert!(super::edit_query(&mut query, event)); }
        assert_eq!(query, "a世界é");
    }
    #[test]
    fn menu_windows_include_wrapped_and_scrolled_selections() {
        for (len, limit) in [(9, 8), (25, 12)] {
            for selected in 0..len {
                let window = super::selection_window(selected, len, limit);
                assert!(window.contains(&selected));
                assert_eq!(window.len(), limit);
            }
        }
        assert!(super::selection_window(0, 0, 8).is_empty());
    }
}
