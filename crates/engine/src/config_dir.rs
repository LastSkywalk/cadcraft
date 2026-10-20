//! The per-user settings directory, one for the desktop app, `cadcraft-cli` and the engine's own
//! stores. It holds the workspace file, `logs/`, `autosave/`, `support/` (startup LISP),
//! `plotters/` (plotter configurations) and `fonts/` (the user's own fonts).

use std::ffi::OsString;
use std::path::PathBuf;

/// The per-user settings directory: `CADCRAFT_CONFIG_DIR` if set, else
/// `$XDG_CONFIG_HOME/cadcraft` or `~/.config/cadcraft` on Linux and the BSDs,
/// `~/Library/Application Support/CADCraft` on macOS, `%APPDATA%\CADCraft` on Windows.
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(std::env::consts::OS, |key| std::env::var_os(key))
}

/// The folder of the user's own fonts in the settings directory.
pub fn user_font_dirs() -> Vec<PathBuf> {
    config_dir().map(|d| d.join("fonts")).into_iter().collect()
}

/// [`config_dir`] on `platform` (`std::env::consts::OS`) with the environment passed in (`get`
/// looks a variable up). An empty `CADCRAFT_CONFIG_DIR` turns the settings directory off; any
/// other empty variable counts as unset, so a blank value never means the working directory. A
/// relative `XDG_CONFIG_HOME` is ignored, as the XDG Base Directory specification says.
pub fn config_dir_from(platform: &str, get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(root) = get("CADCRAFT_CONFIG_DIR") {
        return (!root.is_empty()).then(|| PathBuf::from(root));
    }
    let nonempty = |key| get(key).filter(|value| !value.is_empty()).map(PathBuf::from);
    match platform {
        "macos" => nonempty("HOME").map(|home| home.join("Library/Application Support/CADCraft")),
        "windows" => nonempty("APPDATA").map(|root| root.join("CADCraft")),
        _ => nonempty("XDG_CONFIG_HOME")
            .filter(|root| root.is_absolute())
            .map(|root| root.join("cadcraft"))
            .or_else(|| nonempty("HOME").map(|home| home.join(".config/cadcraft"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn profile_override_isolated_or_explicitly_disabled() {
        let get = |key: &str| match key {
            "CADCRAFT_CONFIG_DIR" => Some(OsString::from("/private/test-state")),
            "HOME" | "APPDATA" | "XDG_CONFIG_HOME" => Some(OsString::from("/unrelated-user-profile")),
            _ => None,
        };
        for platform in ["macos", "windows", "linux", "freebsd"] {
            assert_eq!(config_dir_from(platform, get), Some(PathBuf::from("/private/test-state")));
            assert_eq!(config_dir_from(platform, |key| if key == "CADCRAFT_CONFIG_DIR" { Some(OsString::new()) } else { get(key) }), None);
        }
        assert_eq!(config_dir_from("macos", |_| None), None);
    }

    #[test]
    fn the_override_variable_wins_and_empty_values_are_unset() {
        let vars = [("CADCRAFT_CONFIG_DIR", "/tmp/cc"), ("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/x"), ("APPDATA", "C:\\a")];
        let empty = [("CADCRAFT_CONFIG_DIR", ""), ("HOME", ""), ("XDG_CONFIG_HOME", ""), ("APPDATA", "")];
        let unset = [("HOME", ""), ("XDG_CONFIG_HOME", ""), ("APPDATA", "")];
        for platform in ["macos", "windows", "linux", "freebsd"] {
            assert_eq!(config_dir_from(platform, env(&vars)), Some(PathBuf::from("/tmp/cc")));
            assert_eq!(config_dir_from(platform, env(&empty)), None, "no usable variable means no directory, not the working directory");
            assert_eq!(config_dir_from(platform, env(&unset)), None, "{platform}");
            assert_eq!(config_dir_from(platform, env(&[])), None);
        }
    }

    // `/x` is an absolute path on the hosts these platforms run on.
    #[cfg(unix)]
    #[test]
    fn linux_and_bsd_follow_xdg_config_home() {
        assert_eq!(
            config_dir_from("linux", |key| (key == "XDG_CONFIG_HOME").then(|| OsString::from("/config"))),
            Some(PathBuf::from("/config/cadcraft"))
        );
        for platform in ["linux", "freebsd"] {
            assert_eq!(config_dir_from(platform, env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/home/u")])), Some(PathBuf::from("/x/cadcraft")));
            assert_eq!(config_dir_from(platform, env(&[("HOME", "/home/u")])), Some(PathBuf::from("/home/u/.config/cadcraft")));
            // The XDG spec says a relative XDG_CONFIG_HOME is ignored.
            assert_eq!(
                config_dir_from(platform, env(&[("XDG_CONFIG_HOME", "rel"), ("HOME", "/home/u")])),
                Some(PathBuf::from("/home/u/.config/cadcraft"))
            );
        }
    }

    #[test]
    fn macos_uses_application_support() {
        assert_eq!(config_dir_from("macos", env(&[("HOME", "/Users/u")])), Some(PathBuf::from("/Users/u/Library/Application Support/CADCraft")));
    }

    #[test]
    fn windows_uses_appdata() {
        assert_eq!(
            config_dir_from("windows", env(&[("APPDATA", "C:\\Users\\u\\AppData\\Roaming")])),
            Some(PathBuf::from("C:\\Users\\u\\AppData\\Roaming").join("CADCraft"))
        );
    }
}
