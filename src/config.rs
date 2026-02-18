//! User configuration — keybindings and persistence.
//!
//! Bindings are stored as a simple key-value text file at
//! `$XDG_CONFIG_HOME/dir-tree/config.toml` (default `~/.config/dir-tree/config.toml`).

use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// ───────────────────────────────────────── actions ───────────

/// All configurable user actions in the tree view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    MoveUp,
    MoveDown,
    Expand,
    Collapse,
    JumpSiblingUp,
    JumpSiblingDown,
    CdIntoDir,
    ToggleHidden,
    OpenSettings,
    Quit,
}

impl Action {
    /// Ordered list of all actions (used for the controls menu).
    pub const ALL: &[Action] = &[
        Action::MoveUp,
        Action::MoveDown,
        Action::Expand,
        Action::Collapse,
        Action::JumpSiblingUp,
        Action::JumpSiblingDown,
        Action::CdIntoDir,
        Action::ToggleHidden,
        Action::OpenSettings,
        Action::Quit,
    ];

    /// Human-readable label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            Action::MoveUp => "Move Up",
            Action::MoveDown => "Move Down",
            Action::Expand => "Expand",
            Action::Collapse => "Collapse / Parent",
            Action::JumpSiblingUp => "Prev Sibling Dir",
            Action::JumpSiblingDown => "Next Sibling Dir",
            Action::CdIntoDir => "Enter Directory",
            Action::ToggleHidden => "Toggle Hidden",
            Action::OpenSettings => "Open Settings",
            Action::Quit => "Quit",
        }
    }

    /// Key used in the config file.
    fn config_key(self) -> &'static str {
        match self {
            Action::MoveUp => "move_up",
            Action::MoveDown => "move_down",
            Action::Expand => "expand",
            Action::Collapse => "collapse",
            Action::JumpSiblingUp => "jump_sibling_up",
            Action::JumpSiblingDown => "jump_sibling_down",
            Action::CdIntoDir => "enter_dir",
            Action::ToggleHidden => "toggle_hidden",
            Action::OpenSettings => "open_settings",
            Action::Quit => "quit",
        }
    }

    fn from_config_key(s: &str) -> Option<Self> {
        match s {
            "move_up" => Some(Action::MoveUp),
            "move_down" => Some(Action::MoveDown),
            "expand" => Some(Action::Expand),
            "collapse" => Some(Action::Collapse),
            "jump_sibling_up" => Some(Action::JumpSiblingUp),
            "jump_sibling_down" => Some(Action::JumpSiblingDown),
            "enter_dir" => Some(Action::CdIntoDir),
            "toggle_hidden" => Some(Action::ToggleHidden),
            "open_settings" => Some(Action::OpenSettings),
            "quit" => Some(Action::Quit),
            _ => None,
        }
    }
}

// ───────────────────────────────────────── key bind ──────────

/// A single key binding — key code + modifier combination.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyBind {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBind {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    /// Does this binding match a key event?  Only CTRL/ALT/SHIFT modifiers
    /// are compared (platform-specific modifiers like SUPER are ignored).
    pub fn matches(&self, event: KeyEvent) -> bool {
        let mask = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT;
        self.code == event.code && (self.modifiers & mask) == (event.modifiers & mask)
    }

    /// Create a binding from a raw key event (used during rebinding).
    pub fn from_key_event(event: KeyEvent) -> Self {
        let mask = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT;
        Self {
            code: event.code,
            modifiers: event.modifiers & mask,
        }
    }

    /// User-friendly display string (e.g. `"Alt+↑"`, `"Ctrl+c"`, `"q"`).
    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            s.push_str("Ctrl+");
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            s.push_str("Alt+");
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            s.push_str("Shift+");
        }
        s.push_str(&match self.code {
            KeyCode::Char(' ') => "Space".into(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::Enter => "Enter".into(),
            KeyCode::Esc => "Esc".into(),
            KeyCode::Tab => "Tab".into(),
            KeyCode::Backspace => "Bksp".into(),
            KeyCode::Delete => "Del".into(),
            KeyCode::Home => "Home".into(),
            KeyCode::End => "End".into(),
            KeyCode::PageUp => "PgUp".into(),
            KeyCode::PageDown => "PgDn".into(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        });
        s
    }

    /// Serialise to config-file format (e.g. `"Alt+Up"`, `"Ctrl+c"`, `"q"`).
    fn to_config_string(&self) -> String {
        let mut s = String::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            s.push_str("Ctrl+");
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            s.push_str("Alt+");
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            s.push_str("Shift+");
        }
        s.push_str(&match self.code {
            KeyCode::Char(' ') => "Space".into(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Up => "Up".into(),
            KeyCode::Down => "Down".into(),
            KeyCode::Left => "Left".into(),
            KeyCode::Right => "Right".into(),
            KeyCode::Enter => "Enter".into(),
            KeyCode::Esc => "Esc".into(),
            KeyCode::Tab => "Tab".into(),
            KeyCode::Backspace => "Backspace".into(),
            KeyCode::Delete => "Delete".into(),
            KeyCode::Home => "Home".into(),
            KeyCode::End => "End".into(),
            KeyCode::PageUp => "PageUp".into(),
            KeyCode::PageDown => "PageDown".into(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        });
        s
    }

    /// Parse a key string like `"Ctrl+c"`, `"Alt+Up"`, `"q"`, `"Enter"`.
    fn parse(s: &str) -> Option<Self> {
        let mut modifiers = KeyModifiers::NONE;
        let parts: Vec<&str> = s.split('+').collect();
        let key_part = parts.last()?;

        for &part in &parts[..parts.len() - 1] {
            match part.to_lowercase().as_str() {
                "ctrl" => modifiers |= KeyModifiers::CONTROL,
                "alt" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                _ => return None,
            }
        }

        let code = match key_part.to_lowercase().as_str() {
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" | "bksp" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            s if s.starts_with('f') && s.len() > 1 => {
                let n: u8 = s[1..].parse().ok()?;
                KeyCode::F(n)
            }
            s if s.len() == 1 => KeyCode::Char(s.chars().next()?),
            _ => return None,
        };

        Some(KeyBind { code, modifiers })
    }
}

// ───────────────────────────────────────── config ────────────

/// Application configuration — keybindings and walk settings.
pub struct AppConfig {
    pub bindings: HashMap<Action, Vec<KeyBind>>,
    /// Deduplicate hard links in size computation.
    pub dedup_hard_links: bool,
    /// Stay on the same filesystem (don't cross mount points).
    pub one_file_system: bool,
    /// Double-click detection window for mouse directory activation.
    pub double_click_ms: u64,
}

impl AppConfig {
    /// Hard-coded defaults matching the original keybindings.
    pub fn default_bindings() -> HashMap<Action, Vec<KeyBind>> {
        use Action::*;
        use KeyCode::*;
        let n = KeyModifiers::NONE;
        let alt = KeyModifiers::ALT;
        let mut m = HashMap::new();

        m.insert(MoveUp, vec![KeyBind::new(Up, n), KeyBind::new(Char('k'), n)]);
        m.insert(MoveDown, vec![KeyBind::new(Down, n), KeyBind::new(Char('j'), n)]);
        m.insert(Expand, vec![KeyBind::new(Right, n), KeyBind::new(Char('l'), n)]);
        m.insert(Collapse, vec![KeyBind::new(Left, n), KeyBind::new(Char('h'), n)]);
        m.insert(JumpSiblingUp, vec![KeyBind::new(Up, alt)]);
        m.insert(JumpSiblingDown, vec![KeyBind::new(Down, alt)]);
        m.insert(CdIntoDir, vec![KeyBind::new(Enter, n)]);
        m.insert(ToggleHidden, vec![KeyBind::new(Char('.'), n)]);
        m.insert(OpenSettings, vec![KeyBind::new(Char('?'), n)]);
        m.insert(Quit, vec![KeyBind::new(Char('q'), n)]);

        m
    }

    /// Find the action that matches a key event.  When multiple bindings
    /// match (shouldn't happen after conflict resolution), the one with
    /// the most modifiers wins.
    pub fn match_key(&self, event: KeyEvent) -> Option<Action> {
        let mut best: Option<Action> = None;
        let mut best_mod_count = 0;

        for (&action, binds) in &self.bindings {
            for bind in binds {
                if bind.matches(event) {
                    let mc = bind.modifiers.bits().count_ones();
                    if best.is_none() || mc > best_mod_count {
                        best = Some(action);
                        best_mod_count = mc;
                    }
                }
            }
        }
        best
    }

    /// Add a binding for `action`.  Removes this key from any other action
    /// to prevent conflicts, then appends it to `action`'s bindings.
    pub fn add_binding(&mut self, action: Action, bind: KeyBind) {
        for (_, binds) in self.bindings.iter_mut() {
            binds.retain(|b| b != &bind);
        }
        self.bindings.entry(action).or_default().push(bind);
    }

    /// Restore all bindings to the built-in defaults.
    pub fn reset_defaults(&mut self) {
        self.bindings = Self::default_bindings();
    }

    /// Format the binding list for a given action (e.g. `"↑ / k"`).
    pub fn display_bindings(&self, action: Action) -> String {
        match self.bindings.get(&action) {
            Some(binds) if !binds.is_empty() => {
                binds.iter().map(|b| b.display()).collect::<Vec<_>>().join("/")
            }
            _ => "unbound".into(),
        }
    }

    /// Short display of the first binding only (for the status bar).
    fn short_binding(&self, action: Action) -> String {
        match self.bindings.get(&action) {
            Some(binds) if !binds.is_empty() => binds[0].display(),
            _ => "?".into(),
        }
    }

    /// Build the status-bar hint string from current bindings.
    pub fn status_bar_hint(&self) -> String {
        format!(
            "{}: navigate | {}: expand/collapse | {}: cd | {}: settings",
            self.short_binding(Action::MoveUp),
            self.short_binding(Action::Expand),
            self.short_binding(Action::CdIntoDir),
            self.short_binding(Action::OpenSettings),
        )
    }

    // ── persistence ─────────────────────────────────────────────

    /// Load config from disk, falling back to defaults.
    pub fn load() -> Self {
        let path = config_path();
        if path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                let (bindings, dedup, ofs, dclick_ms) = Self::parse_config(&contents);
                return Self {
                    bindings,
                    dedup_hard_links: dedup,
                    one_file_system: ofs,
                    double_click_ms: dclick_ms,
                };
            }
        }
        Self {
            bindings: Self::default_bindings(),
            dedup_hard_links: true,
            one_file_system: false,
            double_click_ms: 250,
        }
    }

    /// Persist current config to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, self.serialise())?;
        Ok(())
    }

    fn parse_config(s: &str) -> (HashMap<Action, Vec<KeyBind>>, bool, bool, u64) {
        let mut bindings = Self::default_bindings();
        let mut dedup_hard_links = true;
        let mut one_file_system = false;
        let mut double_click_ms = 250;

        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();

            // Walk settings.
            match key {
                "dedup_hard_links" => {
                    dedup_hard_links = value == "true";
                    continue;
                }
                "one_file_system" => {
                    one_file_system = value == "true";
                    continue;
                }
                "double_click_ms" => {
                    if let Ok(v) = value.parse::<u64>() {
                        // Keep this bounded for predictable UX.
                        double_click_ms = v.clamp(100, 2000);
                    }
                    continue;
                }
                _ => {}
            }

            let Some(action) = Action::from_config_key(key) else {
                continue;
            };

            let mut parsed = Vec::new();
            for part in value.split(',') {
                let part = part.trim().trim_matches('"');
                if let Some(bind) = KeyBind::parse(part) {
                    parsed.push(bind);
                }
            }
            if !parsed.is_empty() {
                bindings.insert(action, parsed);
            }
        }

        (bindings, dedup_hard_links, one_file_system, double_click_ms)
    }

    fn serialise(&self) -> String {
        let mut lines = vec![
            "# dir-tree configuration".to_string(),
            String::new(),
            "# Walk settings".to_string(),
            format!("dedup_hard_links = {}", self.dedup_hard_links),
            format!("one_file_system = {}", self.one_file_system),
            format!("double_click_ms = {}", self.double_click_ms),
            String::new(),
            "# Key bindings".to_string(),
            "# Format: action = Key1, Key2, ...".to_string(),
            "# Modifiers: Ctrl+, Alt+, Shift+ (prefix)".to_string(),
            "# Special keys: Up, Down, Left, Right, Enter, Esc, Tab,".to_string(),
            "#   Backspace, Delete, Home, End, PageUp, PageDown, Space, F1-F12".to_string(),
            String::new(),
        ];

        for &action in Action::ALL {
            if let Some(binds) = self.bindings.get(&action) {
                let keys: Vec<String> = binds.iter().map(|b| b.to_config_string()).collect();
                lines.push(format!("{} = {}", action.config_key(), keys.join(", ")));
            }
        }
        lines.push(String::new());
        lines.join("\n")
    }
}

/// Return the config file path (`$XDG_CONFIG_HOME/dir-tree/config.toml`).
fn config_path() -> PathBuf {
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".config")
        });
    config_dir.join("dir-tree").join("config.toml")
}

