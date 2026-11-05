//! Embedded crate configuration: typed parsing and compatibility validation.
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    jni_version: JniVersion,
}

#[derive(Debug, Deserialize)]
enum JniVersion {
    #[serde(rename = "1.8")]
    V1_8,
}

impl Config {
    pub(crate) fn jni_version(&self) -> jni::sys::jint {
        match self.jni_version {
            JniVersion::V1_8 => jni::sys::JNI_VERSION_1_8,
        }
    }
}

pub(crate) fn get() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| {
        toml::from_str(include_str!("../config.toml")).expect("invalid embedded config.toml")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_configuration_is_valid_and_cached() {
        assert_eq!(get().jni_version(), jni::sys::JNI_VERSION_1_8);
        assert!(std::ptr::eq(get(), get()));
    }

    #[test]
    fn rejects_invalid_configuration() {
        for input in [
            "",
            "[compatibility]\njni_version = \"1.8\"",
            "jni_version =",
            "jni_version = 8",
            "jni_version = \"21\"",
            "jni_version = \"1.6\"",
            "jni_version = \"1.8\"\njni_version = \"1.8\"",
            "jni_version = \"1.8\"\nunknown = true",
            "jni_version = \"1.8\"\n[unknown]",
        ] {
            assert!(toml::from_str::<Config>(input).is_err(), "{input}");
        }
    }

    #[test]
    fn accepts_toml_comments_and_literal_strings() {
        let config: Config =
            toml::from_str("# JNI interface\njni_version = '1.8' # baseline\n").unwrap();
        assert_eq!(config.jni_version(), jni::sys::JNI_VERSION_1_8);
    }
}
