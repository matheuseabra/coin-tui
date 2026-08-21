//! Keyboard policy and pure table-navigation transitions.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub(crate) fn should_quit(key: KeyEvent, typing: bool) -> bool {
    key.kind == KeyEventKind::Press
        && (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
            || (key.code == KeyCode::Char('q') && !typing))
}

pub(crate) fn is_search_start(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && key.code == KeyCode::Char('/') && key.modifiers.is_empty()
}

pub(crate) fn is_help_toggle(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && key.code == KeyCode::Char('?')
        && (key.modifiers.is_empty() || key.modifiers.contains(KeyModifiers::SHIFT))
}

pub(crate) fn is_esc(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && key.code == KeyCode::Esc && key.modifiers.is_empty()
}

pub(crate) fn is_detail_open(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && key.code == KeyCode::Enter && key.modifiers.is_empty()
}

pub(crate) fn is_pane_forward(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && key.code == KeyCode::Tab
        && !key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT)
}

pub(crate) fn is_pane_backward(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && (key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT)))
}

pub(crate) fn is_sort_forward(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && key.code == KeyCode::Char('s') && key.modifiers.is_empty()
}

pub(crate) fn is_sort_backward(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && (key.code == KeyCode::Char('S')
            || (key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::SHIFT)))
}

pub(crate) fn clear_active_search(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && key.code == KeyCode::Esc && key.modifiers.is_empty()
}

pub(crate) fn is_refresh(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && key.code == KeyCode::Char('r') && key.modifiers.is_empty()
}

pub(crate) fn is_theme_forward(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && key.code == KeyCode::Char('t') && key.modifiers.is_empty()
}

pub(crate) fn is_theme_backward(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && (key.code == KeyCode::Char('T')
            || (key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::SHIFT)))
}

pub(crate) fn navigation_key(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Down
            | KeyCode::Up
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Char('j')
            | KeyCode::Char('k')
            | KeyCode::Char('g')
            | KeyCode::Char('G')
    )
}

pub(crate) fn navigation_target(
    code: KeyCode,
    current: usize,
    count: usize,
    viewport: usize,
) -> usize {
    if count == 0 {
        return 0;
    }
    let last = count - 1;
    match code {
        KeyCode::PageDown => current.saturating_add(viewport).min(last),
        KeyCode::Down | KeyCode::Char('j') => current.saturating_add(1).min(last),
        KeyCode::PageUp => current.saturating_sub(viewport),
        KeyCode::Up | KeyCode::Char('k') => current.saturating_sub(1),
        KeyCode::Home | KeyCode::Char('g') => 0,
        KeyCode::End | KeyCode::Char('G') => last,
        _ => current.min(last),
    }
}

pub(crate) fn table_viewport(height: u16) -> usize {
    height.saturating_sub(7).div_ceil(2).max(1) as usize
}
