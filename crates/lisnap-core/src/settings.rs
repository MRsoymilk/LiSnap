//! Persistent user preferences shared by the CLI and graphical settings window.
//! This small, stable TOML subset keeps core and headless builds dependency-free.
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AppSettings {
    /// Include the compositor-rendered pointer in captured screen pixels.
    /// The historical/default behavior is to omit it.
    pub include_cursor: bool,
}

impl AppSettings {
    pub fn config_path() -> Result<PathBuf, String> {
        if let Some(base) = env::var_os("XDG_CONFIG_HOME")
            && !base.is_empty()
            && Path::new(&base).is_absolute()
        {
            return Ok(PathBuf::from(base).join("lisnap").join("config.toml"));
        }
        env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(|home| PathBuf::from(home).join(".config").join("lisnap").join("config.toml"))
            .ok_or_else(|| "cannot locate LiSnap settings: set HOME or an absolute XDG_CONFIG_HOME".into())
    }

    pub fn load() -> Result<Self, String> {
        Self::load_from(&Self::config_path()?)
    }

    pub fn load_from(path: &Path) -> Result<Self, String> {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("reading settings {} failed: {error}", path.display())),
        };
        Self::parse(&contents).map_err(|error| format!("invalid settings {}: {error}", path.display()))
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::config_path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("creating settings directory {} failed: {error}", parent.display()))?;
        }
        let content = format!(
            "# LiSnap user settings. Changes can also be made with 'lisnap --settings'.\n[capture]\ninclude_cursor = {}\n",
            self.include_cursor,
        );
        fs::write(path, content).map_err(|error| format!("saving settings {} failed: {error}", path.display()))
    }

    pub fn include_cursor_with_override(self, override_value: Option<bool>) -> bool {
        override_value.unwrap_or(self.include_cursor)
    }

    fn parse(contents: &str) -> Result<Self, String> {
        let mut settings = Self::default();
        let mut section = "";
        let mut seen_cursor = false;
        for (index, raw_line) in contents.trim_start_matches('\u{feff}').lines().enumerate() {
            let line = raw_line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                if !line.ends_with(']') {
                    return Err(format!("line {}: incomplete section name", index + 1));
                }
                section = line[1..line.len() - 1].trim();
                continue;
            }
            if section != "capture" {
                continue;
            }
            let (key, value) =
                line.split_once('=').ok_or_else(|| format!("line {}: expected key = value", index + 1))?;
            if key.trim() != "include_cursor" {
                continue;
            }
            if seen_cursor {
                return Err(format!("line {}: duplicate capture.include_cursor", index + 1));
            }
            settings.include_cursor = match value.trim() {
                "true" => true,
                "false" => false,
                _ => return Err(format!("line {}: capture.include_cursor must be true or false", index + 1)),
            };
            seen_cursor = true;
        }
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_does_not_include_cursor() {
        assert!(!AppSettings::default().include_cursor);
        assert!(!AppSettings::default().include_cursor_with_override(None));
        assert!(AppSettings::default().include_cursor_with_override(Some(true)));
        assert!(!AppSettings { include_cursor: true }.include_cursor_with_override(Some(false)));
    }

    #[test]
    fn parses_saved_toml_and_ignores_unrelated_sections() {
        let text = "# comment\n[appearance]\ntheme = \"dark\"\n[capture]\ninclude_cursor = true # inline comment\n";
        assert_eq!(AppSettings::parse(text).unwrap(), AppSettings { include_cursor: true });
        assert_eq!(AppSettings::parse("[capture]\ninclude_cursor = false\n").unwrap(), AppSettings::default());
        assert_eq!(AppSettings::parse("[capture]\nfuture_setting = 1\n").unwrap(), AppSettings::default());
    }

    #[test]
    fn rejects_duplicate_or_invalid_cursor_settings() {
        for text in [
            "[capture]\ninclude_cursor = yes",
            "[capture]\ninclude_cursor = 1",
            "[capture]\ninclude_cursor = true\ninclude_cursor = false",
            "[capture",
        ] {
            assert!(AppSettings::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn saves_and_loads_persistent_setting() {
        let path = std::env::temp_dir()
            .join(format!(
                "lisnap-settings-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            ))
            .join("config.toml");
        assert_eq!(AppSettings::load_from(&path).unwrap(), AppSettings::default());
        let settings = AppSettings { include_cursor: true };
        settings.save_to(&path).unwrap();
        assert_eq!(AppSettings::load_from(&path).unwrap(), settings);
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("[capture]\ninclude_cursor = true"));
        AppSettings::default().save_to(&path).unwrap();
        assert_eq!(AppSettings::load_from(&path).unwrap(), AppSettings::default());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn reports_invalid_file_instead_of_silently_changing_user_preference() {
        let path = std::env::temp_dir().join(format!(
            "lisnap-invalid-settings-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::write(&path, "[capture]\ninclude_cursor = unsupported\n").unwrap();
        let error = AppSettings::load_from(&path).unwrap_err();
        assert!(error.contains("include_cursor"));
        fs::remove_file(path).unwrap();
    }
}
